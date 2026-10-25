//! Printable bytecode text has the same delimiter semantics for PUSH and data candidates.

/// Yield printable UTF-8 byte ranges; invalid bytes and controls delimit runs.
pub(crate) fn printable_text_runs(bytes: &[u8]) -> impl Iterator<Item = (usize, &str)> {
    let mut cursor = 0;
    let mut base = 0;
    let mut valid = "";
    let mut characters = valid.char_indices();
    let mut run = None;
    std::iter::from_fn(move || {
        loop {
            for (offset, character) in characters.by_ref() {
                if character.is_control() {
                    if let Some(begin) = run.take() {
                        return Some((base + begin, &valid[begin..offset]));
                    }
                } else if run.is_none() {
                    run = Some(offset);
                }
            }
            if let Some(begin) = run.take() {
                return Some((base + begin, &valid[begin..]));
            }
            if cursor == bytes.len() {
                return None;
            }
            base = cursor;
            valid = match std::str::from_utf8(&bytes[cursor..]) {
                Ok(text) => text,
                Err(error) => {
                    std::str::from_utf8(&bytes[cursor..cursor + error.valid_up_to()]).unwrap()
                }
            };
            characters = valid.char_indices();
            cursor += valid.len();
            if cursor < bytes.len() {
                cursor += 1;
            }
        }
    })
}
