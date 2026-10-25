//! Self-made fixtures migrated from rasc; Apache-2.0, see rxd-dex/PROVENANCE.md.
#![allow(dead_code)]

fn write_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn push_uleb(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

/// A valid DEX in which every class has one method that loads "Authorization".
/// A minimal stored ZIP. CRC fields are zero because rasc does not read them.
pub fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    zip(entries, false)
}

/// A minimal deflated ZIP for inflate-cache integration and benchmark fixtures.
pub fn deflated_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    zip(entries, true)
}

fn zip(entries: &[(&str, &[u8])], deflated: bool) -> Vec<u8> {
    let mut local = Vec::new();
    let mut central = Vec::new();
    for (name, payload) in entries {
        let compressed = if deflated {
            let mut compressor = flate2::Compress::new(flate2::Compression::best(), false);
            let mut out = vec![0; payload.len() * 2 + 1024];
            compressor
                .compress(payload, &mut out, flate2::FlushCompress::Finish)
                .unwrap();
            out.truncate(compressor.total_out() as usize);
            out
        } else {
            payload.to_vec()
        };
        let method = if deflated { 8u16 } else { 0u16 };
        let offset = local.len() as u32;
        local.extend_from_slice(b"PK\x03\x04");
        local.extend_from_slice(&20u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&method.to_le_bytes());
        local.extend_from_slice(&[0; 4]);
        local.extend_from_slice(&0u32.to_le_bytes());
        local.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        local.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        local.extend_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        local.extend_from_slice(&compressed);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&[0; 4]);
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = local.len() as u32;
    let central_size = central.len() as u32;
    local.extend_from_slice(&central);
    local.extend_from_slice(b"PK\x05\x06");
    local.extend_from_slice(&[0; 4]);
    local.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    local.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    local.extend_from_slice(&central_size.to_le_bytes());
    local.extend_from_slice(&central_offset.to_le_bytes());
    local.extend_from_slice(&0u16.to_le_bytes());
    local
}

pub fn dex041_container(class_counts: &[usize]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut headers = Vec::new();
    for &count in class_counts {
        let offset = out.len();
        out.extend_from_slice(&const_string_fixture_041(count, offset as u32));
        out[offset..offset + 8].copy_from_slice(b"dex\n041\0");
        write_u32(&mut out, offset + 0x74, offset as u32);
        headers.push(offset);
    }
    let total = out.len() as u32;
    for header in headers {
        // container_size spans all members; file_size advances to the next header.
        write_u32(&mut out, header + 0x70, total);
    }
    out
}

pub fn const_string_dex(class_count: usize) -> Vec<u8> {
    const_string_fixture_with(class_count, 0, 0x70)
}

fn const_string_fixture_041(class_count: usize, base: u32) -> Vec<u8> {
    const_string_fixture_with(class_count, base, 0x78)
}

fn const_string_fixture_with(class_count: usize, base: u32, header_size: usize) -> Vec<u8> {
    assert!(class_count > 0);
    let mut strings = vec!["Authorization".to_owned()];
    for index in 0..class_count {
        strings.push(format!("LFixture{index};"));
    }
    for index in 0..class_count {
        strings.push(format!("m{index}"));
    }
    // "V" (void) backs the single method prototype; without a proto_ids
    // section the decompiler rejects the fixture before decompiling.
    strings.insert(class_count + 1, "V".to_owned());
    let n_strings = strings.len();
    let n_types = class_count + 2;

    let string_ids_off = header_size;
    let type_ids_off = string_ids_off + n_strings * 4;
    let proto_ids_off = type_ids_off + n_types * 4;
    let method_ids_off = proto_ids_off + 12;
    let class_defs_off = method_ids_off + class_count * 8;
    let data_off = class_defs_off + class_count * 32;

    let mut data = Vec::new();
    let mut string_offsets = Vec::with_capacity(n_strings);
    for value in &strings {
        string_offsets.push(data_off + data.len());
        push_uleb(&mut data, value.len() as u32);
        data.extend_from_slice(value.as_bytes());
        data.push(0);
    }

    let mut class_data_offsets = Vec::with_capacity(class_count);
    for index in 0..class_count {
        while !(data_off + data.len()).is_multiple_of(4) {
            data.push(0);
        }
        let code_off = ((data_off + data.len()) as u32) + base;
        data.extend_from_slice(&1_u16.to_le_bytes()); // registers_size: v0
        data.extend_from_slice(&0_u16.to_le_bytes()); // ins_size
        data.extend_from_slice(&0_u16.to_le_bytes()); // outs_size
        data.extend_from_slice(&0_u16.to_le_bytes()); // tries_size
        data.extend_from_slice(&0_u32.to_le_bytes()); // Two loads in one method distinguish uses from referencing-method counts.
        data.extend_from_slice(&5_u32.to_le_bytes()); // insns_size in code units
        data.extend_from_slice(&0x001a_u16.to_le_bytes()); // const-string v0, #0
        data.extend_from_slice(&0_u16.to_le_bytes());
        data.extend_from_slice(&0x001a_u16.to_le_bytes()); // const-string v0, #0
        data.extend_from_slice(&0_u16.to_le_bytes());
        data.extend_from_slice(&0x000e_u16.to_le_bytes()); // return-void

        class_data_offsets.push(data_off + data.len());
        data.push(0); // static_fields_size
        data.push(0); // instance_fields_size
        data.push(1); // direct_methods_size
        data.push(0); // virtual_methods_size
        push_uleb(&mut data, index as u32); // method_idx_diff
        push_uleb(&mut data, 0); // access_flags
        push_uleb(&mut data, code_off);
    }

    let total = data_off + data.len();
    let mut out = vec![0_u8; total];
    out[..8].copy_from_slice(b"dex\n039\0");
    write_u32(&mut out, 0x20, total as u32);
    write_u32(&mut out, 0x24, header_size as u32);
    write_u32(&mut out, 0x28, 0x1234_5678);
    write_u32(&mut out, 0x38, n_strings as u32);
    write_u32(&mut out, 0x3c, (string_ids_off as u32) + base);
    write_u32(&mut out, 0x40, n_types as u32);
    write_u32(&mut out, 0x44, (type_ids_off as u32) + base);
    write_u32(&mut out, 0x48, 1); // proto_ids_size
    write_u32(&mut out, 0x4c, (proto_ids_off as u32) + base);
    write_u32(&mut out, 0x58, class_count as u32);
    write_u32(&mut out, 0x5c, (method_ids_off as u32) + base);
    write_u32(&mut out, 0x60, class_count as u32);
    write_u32(&mut out, 0x64, (class_defs_off as u32) + base);

    for (index, offset) in string_offsets.iter().enumerate() {
        write_u32(
            &mut out,
            string_ids_off + index * 4,
            (*offset as u32) + base,
        );
    }
    write_u32(&mut out, type_ids_off, 0);
    // type_ids[class_count + 1] -> "V", the prototype's return type.
    write_u32(
        &mut out,
        type_ids_off + (class_count + 1) * 4,
        (1 + class_count) as u32,
    );
    // One prototype: shorty "V", returns void, takes no parameters.
    write_u32(&mut out, proto_ids_off, (1 + class_count) as u32);
    write_u32(&mut out, proto_ids_off + 4, (class_count + 1) as u32);
    write_u32(&mut out, proto_ids_off + 8, 0);
    for (index, class_data_off) in class_data_offsets.iter().enumerate() {
        write_u32(&mut out, type_ids_off + (index + 1) * 4, (index + 1) as u32);

        let method = method_ids_off + index * 8;
        write_u16(&mut out, method, (index + 1) as u16);
        write_u16(&mut out, method + 2, 0);
        write_u32(&mut out, method + 4, (2 + class_count + index) as u32);

        let class_def = class_defs_off + index * 32;
        write_u32(&mut out, class_def, (index + 1) as u32);
        write_u32(&mut out, class_def + 8, u32::MAX);
        write_u32(&mut out, class_def + 16, u32::MAX);
        write_u32(&mut out, class_def + 24, (*class_data_off as u32) + base);
    }
    out[data_off..].copy_from_slice(&data);
    // The decompiler requires the fixture's Adler-32 after the 12-byte prefix.
    let checksum = adler32(&out[12..]);
    write_u32(&mut out, 0x08, checksum);
    out
}

pub fn binary_manifest_with_cdata(cdata: &str) -> Vec<u8> {
    const ANDROID_URI: &str = "http://schemas.android.com/apk/res/android";
    const PACKAGE: &str = "com.example&app\"x";
    let strings = vec![
        "manifest",
        ANDROID_URI,
        "package",
        PACKAGE,
        "versionCode",
        "42",
        "application",
        cdata,
        "icon",
        "protectionLevel",
        "textSize",
        "lineSpacingMultiplier",
        "colorAccent",
        "debuggable",
        "theme",
    ];
    let index = |needle: &str| strings.iter().position(|s| *s == needle).unwrap() as u32;

    fn start_chunk(out: &mut Vec<u8>, typ: u16, header_size: u16) -> usize {
        push_u16(out, typ);
        push_u16(out, header_size);
        push_u32(out, 0);
        out.len() - 4
    }

    fn finish_chunk(out: &mut [u8], size_pos: usize) {
        let size = (out.len() - (size_pos - 4)) as u32;
        out[size_pos..size_pos + 4].copy_from_slice(&size.to_le_bytes());
    }

    fn push_u16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    /// One typed value: `size, res0, type, data`.
    fn push_value(out: &mut Vec<u8>, value_type: u8, data: u32) {
        push_u16(out, 8);
        out.push(0);
        out.push(value_type);
        push_u32(out, data);
    }

    fn push_node_header(out: &mut Vec<u8>, typ: u16) -> usize {
        let size_pos = start_chunk(out, typ, 16);
        push_u32(out, 1); // line number
        push_u32(out, u32::MAX); // comment: none
        size_pos
    }

    fn push_element(out: &mut Vec<u8>, name: u32, attributes: &[(u32, u32, u8, u32)]) -> usize {
        let size_pos = push_node_header(out, 0x0102);
        push_u32(out, u32::MAX); // element namespace: none
        push_u32(out, name);
        push_u16(out, 20); // attribute_start
        push_u16(out, 20); // attribute_size
        push_u16(out, attributes.len() as u16);
        push_u16(out, 0); // id_index
        push_u16(out, 0); // class_index
        push_u16(out, 0); // style_index
        for (ns, name, value_type, data) in attributes {
            push_u32(out, *ns);
            push_u32(out, *name);
            push_u32(out, u32::MAX); // raw value: none
            push_value(out, *value_type, *data);
        }
        size_pos
    }

    // String pool.
    let mut pool = Vec::new();
    let size_pos = start_chunk(&mut pool, 0x0001, 28);
    push_u32(&mut pool, strings.len() as u32);
    push_u32(&mut pool, 0); // style count: the decoder requires zero
    push_u32(&mut pool, 0x0000_0100); // UTF-8 flag
    push_u32(&mut pool, 28 + 4 * strings.len() as u32); // strings start
    push_u32(&mut pool, 0); // style start
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    for string in &strings {
        offsets.push(data.len() as u32);
        // AOSP lengths use a high-bit extended form when one byte is insufficient.
        for length in [string.chars().count(), string.len()] {
            if length >= 0x80 {
                data.push(0x80 | (length >> 8) as u8);
            }
            data.push(length as u8);
        }
        data.extend_from_slice(string.as_bytes());
        data.push(0);
    }
    for offset in &offsets {
        push_u32(&mut pool, *offset);
    }
    pool.extend_from_slice(&data);
    while pool.len() % 4 != 0 {
        pool.push(0);
    }
    finish_chunk(&mut pool, size_pos);

    // Empty resource map: attribute names here are strings, not resource ids.
    let mut map = Vec::new();
    let size_pos = start_chunk(&mut map, 0x0180, 8);
    finish_chunk(&mut map, size_pos);

    // document nodes
    let mut nodes = Vec::new();
    let size_pos = push_element(
        &mut nodes,
        index("manifest"),
        &[
            (u32::MAX, index("package"), 0x03, index(PACKAGE)),
            (index(ANDROID_URI), index("versionCode"), 0x10, 42),
            (index(ANDROID_URI), index("icon"), 0x01, 0x7F0D_0001),
            (
                index(ANDROID_URI),
                index("protectionLevel"),
                0x11,
                0x4000_3FB4,
            ),
            (index(ANDROID_URI), index("textSize"), 0x05, (16 << 8) | 1),
            (
                index(ANDROID_URI),
                index("lineSpacingMultiplier"),
                0x04,
                0x4000_0000,
            ),
            (index(ANDROID_URI), index("colorAccent"), 0x1c, 0xFF11_2233),
            (index(ANDROID_URI), index("debuggable"), 0x12, 1),
            (index(ANDROID_URI), index("theme"), 0x01, 0x0103_0007),
        ],
    );
    finish_chunk(&mut nodes, size_pos);
    let size_pos = push_element(&mut nodes, index("application"), &[]);
    finish_chunk(&mut nodes, size_pos);
    let size_pos = push_node_header(&mut nodes, 0x0104);
    push_u32(&mut nodes, index(cdata));
    push_value(&mut nodes, 0x03, index(cdata));
    finish_chunk(&mut nodes, size_pos);
    for _ in 0..2 {
        let size_pos = push_node_header(&mut nodes, 0x0103);
        push_u32(&mut nodes, u32::MAX);
        push_u32(&mut nodes, u32::MAX);
        finish_chunk(&mut nodes, size_pos);
    }

    let mut out = Vec::new();
    let size_pos = start_chunk(&mut out, 0x0003, 8);
    out.extend_from_slice(&pool);
    out.extend_from_slice(&map);
    out.extend_from_slice(&nodes);
    finish_chunk(&mut out, size_pos);
    out
}

/// Two reference fields: field_ids 0 is static, field_ids 1 is instance slot 0.
pub fn member_dex() -> Vec<u8> {
    let mut data = const_string_dex(2);
    let class_defs = u32::from_le_bytes(data[0x64..0x68].try_into().unwrap()) as usize;
    let class_data =
        u32::from_le_bytes(data[class_defs + 24..class_defs + 28].try_into().unwrap()) as usize;
    let mut original_method = Vec::new();
    let mut at = class_data + 4;
    for _ in 0..3 {
        loop {
            let byte = data[at];
            at += 1;
            original_method.push(byte);
            if byte & 0x80 == 0 {
                break;
            }
        }
    }
    let fields = data.len();
    for name_index in [4u32, 5] {
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&name_index.to_le_bytes());
    }
    let new_class_data = data.len();
    data.extend_from_slice(&[1, 1, 1, 0, 0, 8, 1, 0]);
    data.extend_from_slice(&original_method);
    write_u32(&mut data, 0x50, 2);
    write_u32(&mut data, 0x54, fields as u32);
    write_u32(&mut data, class_defs + 24, new_class_data as u32);
    let size = data.len() as u32;
    write_u32(&mut data, 0x20, size);
    let checksum = adler32(&data[12..]);
    write_u32(&mut data, 8, checksum);
    data
}
