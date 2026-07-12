#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextError {
    InvalidRange,
    NotCharBoundary,
}

#[must_use]
pub fn length(value: &str) -> usize {
    value.chars().count()
}

/// Returns a character-indexed UTF-8 slice without splitting a scalar.
///
/// # Errors
///
/// Returns [`TextError::InvalidRange`] for an overflowing or reversed range,
/// and [`TextError::NotCharBoundary`] when an index is outside the string's
/// character boundaries.
pub fn slice(value: &str, start: usize, length: usize) -> Result<&str, TextError> {
    let start_byte = value
        .char_indices()
        .nth(start)
        .map(|(byte, _)| byte)
        .or_else(|| (start == value.chars().count()).then_some(value.len()));
    let end_index = start.checked_add(length).ok_or(TextError::InvalidRange)?;
    let end_byte = value
        .char_indices()
        .nth(end_index)
        .map(|(byte, _)| byte)
        .or_else(|| (end_index == value.chars().count()).then_some(value.len()));
    match (start_byte, end_byte) {
        (Some(start_byte), Some(end_byte)) if start_byte <= end_byte => {
            Ok(&value[start_byte..end_byte])
        }
        (Some(_), Some(_)) => Err(TextError::InvalidRange),
        _ => Err(TextError::NotCharBoundary),
    }
}
