#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]

use std::cmp::Ordering;

use pop_types::{FloatKind, FloatValue, IntegerKind, IntegerValue, NumericError};

#[test]
fn every_integer_kind_parses_exact_boundaries_without_host_width_loss() {
    for (kind, minimum_text, maximum_text) in [
        (IntegerKind::Int8, "-128", "127"),
        (IntegerKind::Int16, "-32768", "32767"),
        (IntegerKind::Int32, "-2147483648", "2147483647"),
        (
            IntegerKind::Int64,
            "-9223372036854775808",
            "9223372036854775807",
        ),
        (IntegerKind::UInt8, "0", "255"),
        (IntegerKind::UInt16, "0", "65535"),
        (IntegerKind::UInt32, "0", "4294967295"),
        (IntegerKind::UInt64, "0", "18446744073709551615"),
    ] {
        let minimum = IntegerValue::parse_decimal(minimum_text, kind).expect("minimum");
        let maximum = IntegerValue::parse_decimal(maximum_text, kind).expect("maximum");
        assert_eq!(minimum.kind(), kind);
        assert_eq!(maximum.kind(), kind);
        assert_eq!(minimum.to_string(), minimum_text);
        assert_eq!(maximum.to_string(), maximum_text);
    }

    assert_eq!(
        IntegerValue::parse_decimal("128", IntegerKind::Int8),
        Err(NumericError::OutOfRange)
    );
    assert_eq!(
        IntegerValue::parse_decimal("-129", IntegerKind::Int8),
        Err(NumericError::OutOfRange)
    );
    assert_eq!(
        IntegerValue::parse_decimal("256", IntegerKind::UInt8),
        Err(NumericError::OutOfRange)
    );
    assert_eq!(
        IntegerValue::parse_decimal("-1", IntegerKind::UInt8),
        Err(NumericError::OutOfRange)
    );
}

#[test]
fn checked_integer_operations_use_the_declared_width_and_signedness() {
    let int8_max = IntegerValue::parse_decimal("127", IntegerKind::Int8).expect("Int8");
    let int8_one = IntegerValue::parse_decimal("1", IntegerKind::Int8).expect("Int8");
    assert_eq!(int8_max.checked_add(int8_one), Err(NumericError::Overflow));

    let uint8_zero = IntegerValue::parse_decimal("0", IntegerKind::UInt8).expect("UInt8");
    let uint8_one = IntegerValue::parse_decimal("1", IntegerKind::UInt8).expect("UInt8");
    assert_eq!(
        uint8_zero.checked_subtract(uint8_one),
        Err(NumericError::Overflow)
    );

    let uint64_max =
        IntegerValue::parse_decimal("18446744073709551615", IntegerKind::UInt64).expect("UInt64");
    assert_eq!(uint64_max.unsigned(), Some(u64::MAX));
    assert_eq!(uint64_max.signed(), None);
    assert_eq!(
        uint64_max.compare(uint8_one),
        Err(NumericError::KindMismatch)
    );

    let int64_min =
        IntegerValue::parse_decimal("-9223372036854775808", IntegerKind::Int64).expect("Int64");
    let negative_one = IntegerValue::parse_decimal("-1", IntegerKind::Int64).expect("Int64");
    assert_eq!(
        int64_min.checked_divide(negative_one),
        Err(NumericError::Overflow)
    );
    let zero = IntegerValue::parse_decimal("0", IntegerKind::Int64).expect("Int64");
    assert_eq!(
        negative_one.checked_divide(zero),
        Err(NumericError::DivisionByZero)
    );
}

#[test]
fn integer_comparison_preserves_unsigned_values_above_i64_max() {
    let high = IntegerValue::parse_decimal("18446744073709551615", IntegerKind::UInt64)
        .expect("UInt64 max");
    let lower = IntegerValue::parse_decimal("9223372036854775808", IntegerKind::UInt64)
        .expect("UInt64 high bit");

    assert_eq!(high.compare(lower), Ok(Ordering::Greater));
}

#[test]
fn float_values_preserve_ieee_width_rounding_and_division() {
    let float32_large = FloatValue::parse_decimal("16777216", FloatKind::Float32).expect("Float32");
    let float32_one = FloatValue::parse_decimal("1", FloatKind::Float32).expect("Float32");
    assert_eq!(
        float32_large.checked_add(float32_one).expect("Float32 add"),
        float32_large
    );

    let float64_large = FloatValue::parse_decimal("16777216", FloatKind::Float64).expect("Float64");
    let float64_one = FloatValue::parse_decimal("1", FloatKind::Float64).expect("Float64");
    let float64_exact =
        FloatValue::parse_decimal("16777217", FloatKind::Float64).expect("exact Float64");
    assert_eq!(
        float64_large.checked_add(float64_one).expect("Float64 add"),
        float64_exact
    );

    let zero = FloatValue::parse_decimal("0", FloatKind::Float64).expect("Float64");
    let divided = float64_one.checked_divide(zero).expect("IEEE division");
    assert!(divided.as_f64().is_infinite());
    assert_eq!(divided.kind(), FloatKind::Float64);
}

#[test]
fn numeric_conversions_preserve_rounding_truncation_and_checked_ranges() {
    let wide_integer =
        IntegerValue::parse_decimal("9007199254740993", IntegerKind::Int64).expect("wide Int64");
    assert_eq!(
        wide_integer.to_float(FloatKind::Float64),
        FloatValue::parse_decimal("9007199254740992", FloatKind::Float64).expect("rounded Float64")
    );
    let float32_double_rounding_boundary =
        IntegerValue::parse_decimal("18014399583223809", IntegerKind::UInt64)
            .expect("halfway-plus-one UInt64");
    assert_eq!(
        float32_double_rounding_boundary.to_float(FloatKind::Float32),
        FloatValue::Float32((18_014_399_583_223_809_u64 as f32).to_bits()),
        "integer-to-Float32 conversion must not double-round through Float64"
    );
    assert_ne!(
        18_014_399_583_223_809_u64 as f32, 18_014_399_583_223_809_u64 as f64 as f32,
        "the regression value must distinguish direct and double rounding"
    );

    let negative_fraction =
        FloatValue::parse_decimal("-12.75", FloatKind::Float64).expect("Float64");
    assert_eq!(
        negative_fraction
            .to_integer(IntegerKind::Int16)
            .expect("truncated Int16")
            .to_string(),
        "-12"
    );
    let small_negative = FloatValue::parse_decimal("-0.75", FloatKind::Float64).expect("Float64");
    assert_eq!(
        small_negative
            .to_integer(IntegerKind::UInt8)
            .expect("truncation reaches zero")
            .to_string(),
        "0"
    );

    let uint16_max = IntegerValue::parse_decimal("65535", IntegerKind::UInt16).expect("UInt16 max");
    assert_eq!(
        uint16_max.convert(IntegerKind::UInt8),
        Err(NumericError::OutOfRange)
    );
    let negative = IntegerValue::parse_decimal("-1", IntegerKind::Int8).expect("negative Int8");
    assert_eq!(
        negative.convert(IntegerKind::UInt64),
        Err(NumericError::OutOfRange)
    );
    let uint64_max =
        IntegerValue::parse_decimal("18446744073709551615", IntegerKind::UInt64).expect("UInt64");
    assert_eq!(
        uint64_max.convert(IntegerKind::Int64),
        Err(NumericError::OutOfRange)
    );

    for invalid in [
        FloatValue::Float64(f64::NAN.to_bits()),
        FloatValue::Float64(f64::INFINITY.to_bits()),
        FloatValue::parse_decimal("256", FloatKind::Float64).expect("Float64"),
    ] {
        assert_eq!(
            invalid.to_integer(IntegerKind::UInt8),
            Err(NumericError::OutOfRange)
        );
    }
}

#[test]
fn every_fixed_numeric_conversion_pair_obeys_its_portable_boundary_contract() {
    const INTEGER_KINDS: [IntegerKind; 8] = [
        IntegerKind::Int8,
        IntegerKind::Int16,
        IntegerKind::Int32,
        IntegerKind::Int64,
        IntegerKind::UInt8,
        IntegerKind::UInt16,
        IntegerKind::UInt32,
        IntegerKind::UInt64,
    ];
    const FLOAT_KINDS: [FloatKind; 2] = [FloatKind::Float32, FloatKind::Float64];

    for source in INTEGER_KINDS {
        let (minimum, maximum) = integer_boundaries(source);
        for target in INTEGER_KINDS {
            let minimum_result = minimum.convert(target);
            let maximum_result = maximum.convert(target);
            let minimum_fits = !source.is_signed()
                || (target.is_signed() && target.bit_width() >= source.bit_width());
            let maximum_fits = match (source.is_signed(), target.is_signed()) {
                (true, true) | (false, false) => target.bit_width() >= source.bit_width(),
                (true, false) => target.bit_width() >= source.bit_width() - 1,
                (false, true) => target.bit_width() > source.bit_width(),
            };
            assert_eq!(
                minimum_result.is_ok(),
                minimum_fits,
                "{source:?} -> {target:?} minimum"
            );
            assert_eq!(
                maximum_result.is_ok(),
                maximum_fits,
                "{source:?} -> {target:?} maximum"
            );
            assert!(minimum_result.is_ok() || minimum_result == Err(NumericError::OutOfRange));
            assert!(maximum_result.is_ok() || maximum_result == Err(NumericError::OutOfRange));
        }
        for target in FLOAT_KINDS {
            let converted = maximum.to_float(target);
            assert_eq!(converted.kind(), target, "{source:?} -> {target:?}");
            assert!(converted.as_f64().is_finite());
        }
    }

    for source in FLOAT_KINDS {
        for target in INTEGER_KINDS {
            let one = FloatValue::parse_decimal("1.75", source).expect("fraction");
            assert_eq!(
                one.to_integer(target)
                    .expect("positive truncation")
                    .to_string(),
                "1",
                "{source:?} -> {target:?}"
            );
            let negative = FloatValue::parse_decimal("-0.75", source).expect("fraction");
            assert_eq!(
                negative
                    .to_integer(target)
                    .expect("zero after truncation")
                    .to_string(),
                "0",
                "{source:?} -> {target:?}"
            );
            for invalid in [
                FloatValue::Float32(f32::NAN.to_bits()).convert(source),
                FloatValue::Float32(f32::INFINITY.to_bits()).convert(source),
            ] {
                assert_eq!(
                    invalid.to_integer(target),
                    Err(NumericError::OutOfRange),
                    "{source:?} -> {target:?}"
                );
            }

            let upper_exclusive = if target.is_signed() {
                2_f64.powi(i32::from(target.bit_width()) - 1)
            } else {
                2_f64.powi(i32::from(target.bit_width()))
            };
            let upper = match source {
                FloatKind::Float32 => FloatValue::Float32((upper_exclusive as f32).to_bits()),
                FloatKind::Float64 => FloatValue::Float64(upper_exclusive.to_bits()),
            };
            assert_eq!(
                upper.to_integer(target),
                Err(NumericError::OutOfRange),
                "{source:?} -> {target:?} upper bound"
            );
        }
        for target in FLOAT_KINDS {
            let value = FloatValue::parse_decimal("1.5", source).expect("float");
            assert_eq!(
                value.convert(target).kind(),
                target,
                "{source:?} -> {target:?}"
            );
        }
    }
}

fn integer_boundaries(kind: IntegerKind) -> (IntegerValue, IntegerValue) {
    let (minimum, maximum) = match kind {
        IntegerKind::Int8 => ("-128", "127"),
        IntegerKind::Int16 => ("-32768", "32767"),
        IntegerKind::Int32 => ("-2147483648", "2147483647"),
        IntegerKind::Int64 => ("-9223372036854775808", "9223372036854775807"),
        IntegerKind::UInt8 => ("0", "255"),
        IntegerKind::UInt16 => ("0", "65535"),
        IntegerKind::UInt32 => ("0", "4294967295"),
        IntegerKind::UInt64 => ("0", "18446744073709551615"),
    };
    (
        IntegerValue::parse_decimal(minimum, kind).expect("minimum"),
        IntegerValue::parse_decimal(maximum, kind).expect("maximum"),
    )
}

#[test]
fn decimal_separator_spelling_is_checked_before_numeric_parsing() {
    assert!(FloatValue::parse_decimal("1_000.25_5e1_0", FloatKind::Float64).is_ok());
    assert!(IntegerValue::parse_decimal("1_000", IntegerKind::Int64).is_ok());
    for invalid in ["_1.0", "1_.0", "1._0", "1.0_", "1e_2", "1__0"] {
        assert_eq!(
            FloatValue::parse_decimal(invalid, FloatKind::Float64),
            Err(NumericError::InvalidLiteral),
            "{invalid}"
        );
    }
}
