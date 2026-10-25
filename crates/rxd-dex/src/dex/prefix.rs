//! Read only complete DEX string/table prefixes; None requests full parsing.

use super::read_uleb;
use crate::bytes::read_u32;
use anyhow::Result;

const HEADER_SIZE: usize = 0x70;
const STRING_ID_ITEM: usize = 4;
const TYPE_ID_ITEM: usize = 4;
const CLASS_DEF_ITEM: usize = 32;

/// Class descriptors of `prefix`, or `None` when it is too short to hold them all.
pub(crate) fn class_names(prefix: &[u8]) -> Result<Option<Vec<super::ClassName>>> {
    if prefix.len() < HEADER_SIZE || prefix.get(..4) != Some(b"dex\n") {
        return Ok(None);
    }
    let strings_size = read_u32(prefix, 0x38)? as usize;
    let strings_off = read_u32(prefix, 0x3c)? as usize;
    let types_size = read_u32(prefix, 0x40)? as usize;
    let types_off = read_u32(prefix, 0x44)? as usize;
    let classes_size = read_u32(prefix, 0x60)? as usize;
    let classes_off = read_u32(prefix, 0x64)? as usize;

    // Every table has to be present in full before a single descriptor is trusted.
    for end in [
        table_end(strings_off, strings_size, STRING_ID_ITEM),
        table_end(types_off, types_size, TYPE_ID_ITEM),
        table_end(classes_off, classes_size, CLASS_DEF_ITEM),
    ] {
        match end {
            Some(end) if end <= prefix.len() => {}
            _ => return Ok(None),
        }
    }

    let mut names = Vec::with_capacity(classes_size);
    for index in 0..classes_size {
        let class_idx = read_u32(prefix, classes_off + index * CLASS_DEF_ITEM)? as usize;
        if class_idx >= types_size {
            return Ok(None);
        }
        let string_idx = read_u32(prefix, types_off + class_idx * TYPE_ID_ITEM)? as usize;
        if string_idx >= strings_size {
            return Ok(None);
        }
        let data_off = read_u32(prefix, strings_off + string_idx * STRING_ID_ITEM)? as usize;
        let Some(bytes) = string_at(prefix, data_off) else {
            return Ok(None);
        };
        // `superclass_idx` is one more `type_ids` index, and the descriptor path already
        // proves both tables and the string data are inside the prefix.
        let super_idx = read_u32(prefix, classes_off + index * CLASS_DEF_ITEM + 8)?;
        let super_name = if super_idx == u32::MAX {
            None
        } else {
            let super_idx = super_idx as usize;
            if super_idx >= types_size {
                return Ok(None);
            }
            let string_idx = read_u32(prefix, types_off + super_idx * TYPE_ID_ITEM)? as usize;
            if string_idx >= strings_size {
                return Ok(None);
            }
            let data_off = read_u32(prefix, strings_off + string_idx * STRING_ID_ITEM)? as usize;
            let Some(bytes) = string_at(prefix, data_off) else {
                return Ok(None);
            };
            let name = super::mutf8::decode_owned(bytes);
            let internal = name
                .strip_prefix('L')
                .and_then(|name| name.strip_suffix(';'))
                .unwrap_or(name.as_str());
            (internal != "java/lang/Object").then(|| internal.to_owned())
        };
        names.push((super::mutf8::decode_owned(bytes), super_name));
    }
    Ok(Some(names))
}

/// Checked end of string_ids from the header; None for unreadable prefixes.
pub(crate) fn string_ids_end(prefix: &[u8]) -> Option<usize> {
    if prefix.len() < HEADER_SIZE || prefix.get(..4) != Some(b"dex\n") {
        return None;
    }
    let size = read_u32(prefix, 0x38).ok()? as usize;
    let offset = read_u32(prefix, 0x3c).ok()? as usize;
    table_end(offset, size, STRING_ID_ITEM)
}

/// Last string-data offset from complete string_ids; None if incomplete.
pub(crate) fn string_data_end(prefix: &[u8]) -> Option<usize> {
    if prefix.len() < HEADER_SIZE || prefix.get(..4) != Some(b"dex\n") {
        return None;
    }
    let strings_size = read_u32(prefix, 0x38).ok()? as usize;
    let strings_off = read_u32(prefix, 0x3c).ok()? as usize;
    let strings_end = table_end(strings_off, strings_size, STRING_ID_ITEM)?;
    if strings_end > prefix.len() {
        return None;
    }
    let mut end = 0usize;
    for index in 0..strings_size {
        let off = read_u32(prefix, strings_off + index * STRING_ID_ITEM).ok()? as usize;
        end = end.max(off);
    }
    Some(end)
}

fn table_end(offset: usize, count: usize, stride: usize) -> Option<usize> {
    offset.checked_add(count.checked_mul(stride)?)
}

/// The MUTF-8 bytes of the string_data item at `offset`, without its terminator.
fn string_at(data: &[u8], offset: usize) -> Option<&[u8]> {
    let mut cursor = offset;
    read_uleb(data, &mut cursor).ok()?;
    let tail = data.get(cursor..)?;
    let end = memchr::memchr(0, tail)?;
    Some(&tail[..end])
}

/// Whether `prefix` shows that this DEX defines `descriptor`, or `None` when the
/// prefix cannot answer (too short, or a 041 container which the full reader owns).
pub(crate) fn defines_class(prefix: &[u8], descriptor: &[u8]) -> Result<Option<bool>> {
    if prefix.len() < HEADER_SIZE || prefix.get(..4) != Some(b"dex\n") {
        return Ok(None);
    }
    let strings_size = read_u32(prefix, 0x38)? as usize;
    let strings_off = read_u32(prefix, 0x3c)? as usize;
    let types_size = read_u32(prefix, 0x40)? as usize;
    let types_off = read_u32(prefix, 0x44)? as usize;
    let classes_size = read_u32(prefix, 0x60)? as usize;
    let classes_off = read_u32(prefix, 0x64)? as usize;
    for end in [
        table_end(strings_off, strings_size, STRING_ID_ITEM),
        table_end(types_off, types_size, TYPE_ID_ITEM),
        table_end(classes_off, classes_size, CLASS_DEF_ITEM),
    ]
    .into_iter()
    {
        match end {
            Some(end) if end <= prefix.len() => {}
            _ => return Ok(None),
        }
    }
    for index in 0..classes_size {
        let class_idx = read_u32(prefix, classes_off + index * CLASS_DEF_ITEM)? as usize;
        if class_idx >= types_size {
            return Ok(None);
        }
        let string_idx = read_u32(prefix, types_off + class_idx * TYPE_ID_ITEM)? as usize;
        if string_idx >= strings_size {
            return Ok(None);
        }
        let data_off = read_u32(prefix, strings_off + string_idx * STRING_ID_ITEM)? as usize;
        let Some(bytes) = string_at(prefix, data_off) else {
            return Ok(None);
        };
        if bytes == descriptor {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_sizes_come_from_the_header_and_the_string_table() {
        let dex = crate::dex::tests::const_string_fixture(4);
        let strings_size = read_u32(&dex, 0x38).unwrap() as usize;
        let strings_off = read_u32(&dex, 0x3c).unwrap() as usize;
        assert_eq!(
            string_ids_end(&dex).unwrap(),
            strings_off + strings_size * 4
        );

        let end = string_data_end(&dex).unwrap();
        let last = read_u32(&dex, strings_off + (strings_size - 1) * 4).unwrap() as usize;
        assert_eq!(end, last);
        assert!(
            end < dex.len(),
            "the code section must be after the string data"
        );
    }

    #[test]
    fn a_short_prefix_is_never_trusted() {
        let dex = crate::dex::tests::const_string_fixture(4);
        let strings_size = read_u32(&dex, 0x38).unwrap() as usize;
        let strings_off = read_u32(&dex, 0x3c).unwrap() as usize;
        let end = string_data_end(&dex).unwrap();
        assert!(string_data_end(&dex[..8]).is_none());
        assert!(string_data_end(&dex[..strings_off]).is_none());
        assert!(string_ids_end(&[]).is_none());
        // The table alone is enough to size the string data...
        assert_eq!(
            string_data_end(&dex[..strings_off + strings_size * 4]).unwrap(),
            end
        );
        // ...but nothing can be decoded from it yet: the strings themselves are missing.
        assert!(
            class_names(&dex[..strings_off + strings_size * 4])
                .unwrap()
                .is_none()
        );
        // A prefix that reaches past the last string is enough for the descriptors.
        assert!(class_names(&dex[..end + 64]).unwrap().is_some());
    }

    #[test]
    fn a_crafted_table_size_is_rejected_rather_than_allocated() {
        let mut dex = crate::dex::tests::const_string_fixture(2);
        write_u32(&mut dex, 0x38, 0x00ff_ffff);
        assert!(string_ids_end(&dex).is_none() || string_ids_end(&dex).unwrap() > dex.len());
        assert!(string_data_end(&dex).is_none());
    }

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}
