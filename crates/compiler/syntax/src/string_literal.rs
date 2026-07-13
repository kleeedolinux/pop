use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StringLiteralError;

impl fmt::Display for StringLiteralError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid Pop Lang string literal")
    }
}

impl std::error::Error for StringLiteralError {}

/// Decodes one complete quoted or backtick Pop Lang string literal.
///
/// # Errors
///
/// Returns [`StringLiteralError`] for an invalid delimiter, malformed escape,
/// or invalid Unicode scalar.
pub fn decode_string_literal(literal: &str) -> Result<String, StringLiteralError> {
    let bytes = literal.as_bytes();
    let Some(&quote) = bytes.first() else {
        return Err(StringLiteralError);
    };
    let interpolated = quote == b'`';
    if !matches!(quote, b'\'' | b'"' | b'`') || bytes.last() != Some(&quote) {
        return Err(StringLiteralError);
    }
    decode_string_contents(&literal[1..literal.len() - 1], interpolated)
}

pub(crate) fn decode_string_contents(
    contents: &str,
    interpolated: bool,
) -> Result<String, StringLiteralError> {
    let bytes = contents.as_bytes();
    let mut output = String::with_capacity(contents.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'\\' {
            let character = contents[cursor..]
                .chars()
                .next()
                .ok_or(StringLiteralError)?;
            if matches!(character, '\n' | '\r') {
                return Err(StringLiteralError);
            }
            output.push(character);
            cursor += character.len_utf8();
            continue;
        }
        let end = scan_escape(bytes, cursor, interpolated).map_err(|_| StringLiteralError)?;
        let marker = bytes.get(cursor + 1).copied().ok_or(StringLiteralError)?;
        match marker {
            b'\\' => output.push('\\'),
            b'"' => output.push('"'),
            b'\'' => output.push('\''),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'0' => output.push('\0'),
            b'`' => output.push('`'),
            b'{' => output.push('{'),
            b'}' => output.push('}'),
            b'x' => {
                let value = hex_value(&contents[cursor + 2..end]).ok_or(StringLiteralError)?;
                output.push(char::from(
                    u8::try_from(value).map_err(|_| StringLiteralError)?,
                ));
            }
            b'u' => {
                let value = hex_value(&contents[cursor + 3..end - 1]).ok_or(StringLiteralError)?;
                output.push(char::from_u32(value).ok_or(StringLiteralError)?);
            }
            _ => return Err(StringLiteralError),
        }
        cursor = end;
    }
    Ok(output)
}

pub(crate) fn scan_escape(bytes: &[u8], start: usize, interpolated: bool) -> Result<usize, usize> {
    let Some(&marker) = bytes.get(start + 1) else {
        return Err(bytes.len());
    };
    match marker {
        b'\\' | b'"' | b'\'' | b'n' | b'r' | b't' | b'0' => Ok(start + 2),
        b'`' | b'{' | b'}' if interpolated => Ok(start + 2),
        b'x' => {
            let mut cursor = start + 2;
            while cursor < bytes.len() && cursor < start + 4 && bytes[cursor].is_ascii_hexdigit() {
                cursor += 1;
            }
            if cursor == start + 4 {
                Ok(cursor)
            } else {
                Err(cursor)
            }
        }
        b'u' if bytes.get(start + 2) == Some(&b'{') => {
            let mut cursor = start + 3;
            while cursor < bytes.len() && bytes[cursor].is_ascii_hexdigit() && cursor < start + 9 {
                cursor += 1;
            }
            if cursor == start + 3 || bytes.get(cursor) != Some(&b'}') {
                return Err(cursor.min(bytes.len()));
            }
            let Some(value) = hex_value_bytes(&bytes[start + 3..cursor]) else {
                return Err(cursor + 1);
            };
            if char::from_u32(value).is_none() {
                return Err(cursor + 1);
            }
            Ok(cursor + 1)
        }
        _ => Err((start + 2).min(bytes.len())),
    }
}

fn hex_value(text: &str) -> Option<u32> {
    hex_value_bytes(text.as_bytes())
}

fn hex_value_bytes(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a' + 10),
            b'A'..=b'F' => u32::from(byte - b'A' + 10),
            _ => return None,
        };
        value.checked_mul(16)?.checked_add(digit)
    })
}
