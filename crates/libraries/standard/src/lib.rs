//! Rust implementation foundation for the public `Pop.Standard` Bubble.
//!
//! These APIs are intentionally small, typed, and function-first. They are
//! implementation adapters for the public Pop contracts, not a second source
//! language or a universal object layer.

use std::io::Write;

/// Prints one Pop `Int` followed by a newline for the native bootstrap host.
///
/// This fixed ABI adapter is linked by the toolchain and is not resolved from
/// user source by symbol spelling.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_std_print_int(value: i64) {
    let _ = writeln!(std::io::stdout().lock(), "{value}");
}

pub mod math {
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
}

pub mod text {
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
    /// Returns [`TextError::InvalidRange`] for an overflowing or reversed
    /// range, and [`TextError::NotCharBoundary`] when an index is outside the
    /// string's character boundaries.
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
}

pub mod sequence {
    pub fn map<T, U>(values: &[T], mut transform: impl FnMut(&T) -> U) -> Vec<U> {
        values.iter().map(&mut transform).collect()
    }

    pub fn filter<T: Clone>(values: &[T], mut predicate: impl FnMut(&T) -> bool) -> Vec<T> {
        values
            .iter()
            .filter(|value| predicate(value))
            .cloned()
            .collect()
    }

    pub fn fold<T, State>(
        values: &[T],
        initial: State,
        mut combine: impl FnMut(State, &T) -> State,
    ) -> State {
        values.iter().fold(initial, &mut combine)
    }
}
