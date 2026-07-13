use std::cmp::Ordering;
use std::error::Error;
use std::fmt;

use crate::IntegerKind;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NumericConversionKind {
    IntegerToInteger {
        source: IntegerKind,
        target: IntegerKind,
    },
    IntegerToFloat {
        source: IntegerKind,
        target: FloatKind,
    },
    FloatToInteger {
        source: FloatKind,
        target: IntegerKind,
    },
    FloatToFloat {
        source: FloatKind,
        target: FloatKind,
    },
}

impl NumericConversionKind {
    #[must_use]
    pub const fn may_trap(self) -> bool {
        match self {
            Self::IntegerToInteger { source, target } => {
                match (source.is_signed(), target.is_signed()) {
                    (true, true) | (false, false) => target.bit_width() < source.bit_width(),
                    (true, false) => true,
                    (false, true) => target.bit_width() <= source.bit_width(),
                }
            }
            Self::FloatToInteger { .. } => true,
            Self::IntegerToFloat { .. } | Self::FloatToFloat { .. } => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericError {
    InvalidLiteral,
    OutOfRange,
    KindMismatch,
    Overflow,
    DivisionByZero,
}

impl fmt::Display for NumericError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "numeric error: {self:?}")
    }
}

impl Error for NumericError {}

/// One canonical fixed-width integer value.
///
/// `bits` stores the exact two's-complement/unsigned bit pattern for `kind`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IntegerValue {
    kind: IntegerKind,
    bits: u64,
}

impl IntegerValue {
    /// Parses one decimal integer spelling for an exact Pop Lang integer kind.
    ///
    /// # Errors
    ///
    /// Returns [`NumericError::InvalidLiteral`] for malformed decimal text and
    /// [`NumericError::OutOfRange`] when the value is not representable.
    pub fn parse_decimal(text: &str, kind: IntegerKind) -> Result<Self, NumericError> {
        if !valid_digit_separators(text) {
            return Err(NumericError::InvalidLiteral);
        }
        let normalized = text.replace('_', "");
        let (negative, magnitude) = normalized
            .strip_prefix('-')
            .map_or((false, normalized.as_str()), |magnitude| (true, magnitude));
        if magnitude.is_empty() || !magnitude.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(NumericError::InvalidLiteral);
        }
        let magnitude = magnitude
            .parse::<u128>()
            .map_err(|_| NumericError::OutOfRange)?;
        if kind.is_signed() {
            let limit = 1_u128 << (kind.bit_width() - 1);
            if negative {
                if magnitude > limit {
                    return Err(NumericError::OutOfRange);
                }
                let value = i128::try_from(magnitude)
                    .map_err(|_| NumericError::OutOfRange)?
                    .checked_neg()
                    .ok_or(NumericError::OutOfRange)?;
                Self::from_signed(kind, value)
            } else {
                if magnitude >= limit {
                    return Err(NumericError::OutOfRange);
                }
                Self::from_signed(
                    kind,
                    i128::try_from(magnitude).map_err(|_| NumericError::OutOfRange)?,
                )
            }
        } else {
            if negative {
                return Err(NumericError::OutOfRange);
            }
            Self::from_unsigned(kind, magnitude)
        }
    }

    #[must_use]
    pub const fn kind(self) -> IntegerKind {
        self.kind
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        self.bits
    }

    #[must_use]
    pub fn signed(self) -> Option<i64> {
        self.kind.is_signed().then(|| {
            let shift = 64_u32 - u32::from(self.kind.bit_width());
            i64::from_ne_bytes((self.bits << shift).to_ne_bytes()) >> shift
        })
    }

    #[must_use]
    pub const fn unsigned(self) -> Option<u64> {
        if self.kind.is_signed() {
            None
        } else {
            Some(self.bits)
        }
    }

    /// Performs checked addition using this value's declared width.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch or overflow error.
    pub fn checked_add(self, right: Self) -> Result<Self, NumericError> {
        self.checked_binary(right, i128::checked_add, u128::checked_add)
    }

    /// Performs checked subtraction using this value's declared width.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch or overflow error.
    pub fn checked_subtract(self, right: Self) -> Result<Self, NumericError> {
        self.checked_binary(right, i128::checked_sub, u128::checked_sub)
    }

    /// Performs checked multiplication using this value's declared width.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch or overflow error.
    pub fn checked_multiply(self, right: Self) -> Result<Self, NumericError> {
        self.checked_binary(right, i128::checked_mul, u128::checked_mul)
    }

    /// Performs checked division using this value's signedness.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch, division-by-zero, or overflow error.
    pub fn checked_divide(self, right: Self) -> Result<Self, NumericError> {
        self.require_kind(right)?;
        if right.bits == 0 {
            return Err(NumericError::DivisionByZero);
        }
        if self.kind.is_signed() {
            let result = self
                .as_i128()
                .checked_div(right.as_i128())
                .ok_or(NumericError::Overflow)?;
            Self::from_signed(self.kind, result).map_err(|_| NumericError::Overflow)
        } else {
            Self::from_unsigned(self.kind, self.as_u128() / right.as_u128())
                .map_err(|_| NumericError::Overflow)
        }
    }

    /// Performs checked remainder using this value's signedness.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch, division-by-zero, or overflow error.
    pub fn checked_remainder(self, right: Self) -> Result<Self, NumericError> {
        self.require_kind(right)?;
        if right.bits == 0 {
            return Err(NumericError::DivisionByZero);
        }
        if self.kind.is_signed() {
            let result = self
                .as_i128()
                .checked_rem(right.as_i128())
                .ok_or(NumericError::Overflow)?;
            Self::from_signed(self.kind, result).map_err(|_| NumericError::Overflow)
        } else {
            Self::from_unsigned(self.kind, self.as_u128() % right.as_u128())
                .map_err(|_| NumericError::Overflow)
        }
    }

    /// Negates a signed integer using its exact width.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch for unsigned integers and overflow for the
    /// signed minimum value.
    pub fn checked_negate(self) -> Result<Self, NumericError> {
        if !self.kind.is_signed() {
            return Err(NumericError::KindMismatch);
        }
        let value = self.as_i128().checked_neg().ok_or(NumericError::Overflow)?;
        Self::from_signed(self.kind, value).map_err(|_| NumericError::Overflow)
    }

    /// Compares two integers of the same exact kind.
    ///
    /// # Errors
    ///
    /// Returns [`NumericError::KindMismatch`] for different kinds.
    pub fn compare(self, right: Self) -> Result<Ordering, NumericError> {
        self.require_kind(right)?;
        Ok(if self.kind.is_signed() {
            self.as_i128().cmp(&right.as_i128())
        } else {
            self.as_u128().cmp(&right.as_u128())
        })
    }

    /// Converts to another fixed-width integer with a checked target range.
    pub fn convert(self, target: IntegerKind) -> Result<Self, NumericError> {
        if self.kind.is_signed() {
            let value = self.as_i128();
            if target.is_signed() {
                Self::from_signed(target, value)
            } else {
                Self::from_unsigned(
                    target,
                    u128::try_from(value).map_err(|_| NumericError::OutOfRange)?,
                )
            }
        } else if target.is_signed() {
            Self::from_signed(
                target,
                i128::try_from(self.as_u128()).map_err(|_| NumericError::OutOfRange)?,
            )
        } else {
            Self::from_unsigned(target, self.as_u128())
        }
    }

    #[must_use]
    pub fn to_float(self, target: FloatKind) -> FloatValue {
        match target {
            FloatKind::Float32 => {
                let value = if self.kind.is_signed() {
                    self.as_i128() as f32
                } else {
                    self.as_u128() as f32
                };
                FloatValue::Float32(value.to_bits())
            }
            FloatKind::Float64 => {
                let value = if self.kind.is_signed() {
                    self.as_i128() as f64
                } else {
                    self.as_u128() as f64
                };
                FloatValue::Float64(value.to_bits())
            }
        }
    }

    fn checked_binary(
        self,
        right: Self,
        signed: fn(i128, i128) -> Option<i128>,
        unsigned: fn(u128, u128) -> Option<u128>,
    ) -> Result<Self, NumericError> {
        self.require_kind(right)?;
        if self.kind.is_signed() {
            let value = signed(self.as_i128(), right.as_i128()).ok_or(NumericError::Overflow)?;
            Self::from_signed(self.kind, value).map_err(|_| NumericError::Overflow)
        } else {
            let value = unsigned(self.as_u128(), right.as_u128()).ok_or(NumericError::Overflow)?;
            Self::from_unsigned(self.kind, value).map_err(|_| NumericError::Overflow)
        }
    }

    fn require_kind(self, right: Self) -> Result<(), NumericError> {
        if self.kind == right.kind {
            Ok(())
        } else {
            Err(NumericError::KindMismatch)
        }
    }

    fn from_signed(kind: IntegerKind, value: i128) -> Result<Self, NumericError> {
        if !kind.is_signed() {
            return Err(NumericError::KindMismatch);
        }
        let width = kind.bit_width();
        let minimum = -(1_i128 << (width - 1));
        let maximum = (1_i128 << (width - 1)) - 1;
        if !(minimum..=maximum).contains(&value) {
            return Err(NumericError::OutOfRange);
        }
        let encoded = u128::from_le_bytes(value.to_le_bytes()) & u128::from(bit_mask(width));
        Ok(Self {
            kind,
            bits: u64::try_from(encoded).map_err(|_| NumericError::OutOfRange)?,
        })
    }

    fn from_unsigned(kind: IntegerKind, value: u128) -> Result<Self, NumericError> {
        if kind.is_signed() {
            return Err(NumericError::KindMismatch);
        }
        let maximum = u128::from(bit_mask(kind.bit_width()));
        if value > maximum {
            return Err(NumericError::OutOfRange);
        }
        Ok(Self {
            kind,
            bits: u64::try_from(value).map_err(|_| NumericError::OutOfRange)?,
        })
    }

    fn as_i128(self) -> i128 {
        i128::from(self.signed().expect("signed kind has a signed projection"))
    }

    const fn as_u128(self) -> u128 {
        self.bits as u128
    }
}

impl fmt::Display for IntegerValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(value) = self.signed() {
            write!(formatter, "{value}")
        } else {
            write!(formatter, "{}", self.bits)
        }
    }
}

const fn bit_mask(width: u16) -> u64 {
    if width == 64 {
        u64::MAX
    } else {
        (1_u64 << width) - 1
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FloatKind {
    Float32,
    Float64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FloatValue {
    Float32(u32),
    Float64(u64),
}

impl FloatValue {
    /// Parses a decimal spelling into one exact IEEE format.
    ///
    /// # Errors
    ///
    /// Returns an invalid-literal or out-of-range error.
    pub fn parse_decimal(text: &str, kind: FloatKind) -> Result<Self, NumericError> {
        if !valid_digit_separators(text) {
            return Err(NumericError::InvalidLiteral);
        }
        let text = text.replace('_', "");
        match kind {
            FloatKind::Float32 => {
                let value = text
                    .parse::<f32>()
                    .map_err(|_| NumericError::InvalidLiteral)?;
                if !value.is_finite() {
                    return Err(NumericError::OutOfRange);
                }
                Ok(Self::Float32(value.to_bits()))
            }
            FloatKind::Float64 => {
                let value = text
                    .parse::<f64>()
                    .map_err(|_| NumericError::InvalidLiteral)?;
                if !value.is_finite() {
                    return Err(NumericError::OutOfRange);
                }
                Ok(Self::Float64(value.to_bits()))
            }
        }
    }

    #[must_use]
    pub const fn kind(self) -> FloatKind {
        match self {
            Self::Float32(_) => FloatKind::Float32,
            Self::Float64(_) => FloatKind::Float64,
        }
    }

    #[must_use]
    pub fn as_f64(self) -> f64 {
        match self {
            Self::Float32(bits) => f64::from(f32::from_bits(bits)),
            Self::Float64(bits) => f64::from_bits(bits),
        }
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        match self {
            Self::Float32(bits) => bits as u64,
            Self::Float64(bits) => bits,
        }
    }

    /// Formats one IEEE value using Pop Lang's locale-independent string
    /// contract from ADR 0041.
    #[must_use]
    pub fn format_string(self) -> String {
        match self {
            Self::Float32(bits) => format_float32(f32::from_bits(bits)),
            Self::Float64(bits) => format_float64(f64::from_bits(bits)),
        }
    }

    /// Adds equal-format IEEE values.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch for different formats.
    pub fn checked_add(self, right: Self) -> Result<Self, NumericError> {
        self.binary(
            right,
            |left, right| left + right,
            |left, right| left + right,
        )
    }

    /// Subtracts equal-format IEEE values.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch for different formats.
    pub fn checked_subtract(self, right: Self) -> Result<Self, NumericError> {
        self.binary(
            right,
            |left, right| left - right,
            |left, right| left - right,
        )
    }

    /// Multiplies equal-format IEEE values.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch for different formats.
    pub fn checked_multiply(self, right: Self) -> Result<Self, NumericError> {
        self.binary(
            right,
            |left, right| left * right,
            |left, right| left * right,
        )
    }

    /// Divides equal-format IEEE values, including IEEE zero division.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch for different formats.
    pub fn checked_divide(self, right: Self) -> Result<Self, NumericError> {
        self.binary(
            right,
            |left, right| left / right,
            |left, right| left / right,
        )
    }

    /// Negates one IEEE value while preserving its exact format.
    #[must_use]
    pub fn negate(self) -> Self {
        match self {
            Self::Float32(bits) => Self::Float32((-f32::from_bits(bits)).to_bits()),
            Self::Float64(bits) => Self::Float64((-f64::from_bits(bits)).to_bits()),
        }
    }

    /// Partially compares two equal-format IEEE values.
    ///
    /// # Errors
    ///
    /// Returns a kind mismatch for different formats.
    pub fn partial_compare(self, right: Self) -> Result<Option<Ordering>, NumericError> {
        match (self, right) {
            (Self::Float32(left), Self::Float32(right)) => {
                Ok(f32::from_bits(left).partial_cmp(&f32::from_bits(right)))
            }
            (Self::Float64(left), Self::Float64(right)) => {
                Ok(f64::from_bits(left).partial_cmp(&f64::from_bits(right)))
            }
            _ => Err(NumericError::KindMismatch),
        }
    }

    #[must_use]
    pub fn convert(self, target: FloatKind) -> Self {
        if self.kind() == target {
            return self;
        }
        match (self, target) {
            (Self::Float32(bits), FloatKind::Float64) => {
                Self::Float64(f64::from(f32::from_bits(bits)).to_bits())
            }
            (Self::Float64(bits), FloatKind::Float32) => {
                Self::Float32((f64::from_bits(bits) as f32).to_bits())
            }
            _ => self,
        }
    }

    /// Truncates toward zero and checks the complete target integer range.
    pub fn to_integer(self, target: IntegerKind) -> Result<IntegerValue, NumericError> {
        let value = self.as_f64();
        if !value.is_finite() {
            return Err(NumericError::OutOfRange);
        }
        let truncated = value.trunc();
        let width = i32::from(target.bit_width());
        if target.is_signed() {
            let limit = 2_f64.powi(width - 1);
            if truncated < -limit || truncated >= limit {
                return Err(NumericError::OutOfRange);
            }
            IntegerValue::from_signed(target, truncated as i128)
        } else {
            let limit = 2_f64.powi(width);
            if truncated < 0.0 || truncated >= limit {
                return Err(NumericError::OutOfRange);
            }
            IntegerValue::from_unsigned(target, truncated as u128)
        }
    }

    fn binary(
        self,
        right: Self,
        float32: impl FnOnce(f32, f32) -> f32,
        float64: impl FnOnce(f64, f64) -> f64,
    ) -> Result<Self, NumericError> {
        match (self, right) {
            (Self::Float32(left), Self::Float32(right)) => Ok(Self::Float32(
                float32(f32::from_bits(left), f32::from_bits(right)).to_bits(),
            )),
            (Self::Float64(left), Self::Float64(right)) => Ok(Self::Float64(
                float64(f64::from_bits(left), f64::from_bits(right)).to_bits(),
            )),
            _ => Err(NumericError::KindMismatch),
        }
    }
}

fn format_float32(value: f32) -> String {
    if value.is_nan() {
        "nan".to_owned()
    } else if value == f32::INFINITY {
        "inf".to_owned()
    } else if value == f32::NEG_INFINITY {
        "-inf".to_owned()
    } else {
        value.to_string()
    }
}

fn format_float64(value: f64) -> String {
    if value.is_nan() {
        "nan".to_owned()
    } else if value == f64::INFINITY {
        "inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-inf".to_owned()
    } else {
        value.to_string()
    }
}

fn valid_digit_separators(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(index, byte)| {
        *byte != b'_'
            || (index > 0
                && index + 1 < bytes.len()
                && bytes[index - 1].is_ascii_digit()
                && bytes[index + 1].is_ascii_digit())
    })
}
