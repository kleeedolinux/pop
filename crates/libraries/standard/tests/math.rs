use pop_standard::math;

#[test]
fn math_foundation_is_checked_and_typed() {
    assert_eq!(math::checked_add(20_i64, 22_i64), Ok(42));
    assert_eq!(
        math::checked_add(i64::MAX, 1),
        Err(math::MathError::Overflow)
    );
    assert_eq!(math::min(7, 3), 3);
}
