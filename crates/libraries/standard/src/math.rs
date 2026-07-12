#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathError {
    Overflow,
}

/// Adds two `Int` values without wrapping.
///
/// # Errors
///
/// Returns [`MathError::Overflow`] when the exact sum is outside `Int`.
pub fn checked_add(left: i64, right: i64) -> Result<i64, MathError> {
    left.checked_add(right).ok_or(MathError::Overflow)
}

#[must_use]
pub const fn min(left: i64, right: i64) -> i64 {
    if left < right { left } else { right }
}
