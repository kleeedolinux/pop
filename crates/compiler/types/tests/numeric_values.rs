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
