use pop_standard::{math, sequence, text};

#[test]
fn math_foundation_is_checked_and_typed() {
    assert_eq!(math::checked_add(20_i64, 22_i64), Ok(42));
    assert_eq!(
        math::checked_add(i64::MAX, 1),
        Err(math::MathError::Overflow)
    );
    assert_eq!(math::min(7, 3), 3);
}

#[test]
fn text_foundation_preserves_utf8_boundaries() {
    assert_eq!(text::length("Olá"), 3);
    assert_eq!(text::slice("Olá", 1, 2), Ok("lá"));
    assert_eq!(text::slice("Olá", 1, 1), Ok("l"));
    assert_eq!(
        text::slice("Olá", 4, 1),
        Err(text::TextError::NotCharBoundary)
    );
}

#[test]
fn sequence_foundation_is_function_and_data_first() {
    let values = [1, 2, 3, 4];
    assert_eq!(sequence::map(&values, |value| value * 2), vec![2, 4, 6, 8]);
    assert_eq!(
        sequence::filter(&values, |value| *value % 2 == 0),
        vec![2, 4]
    );
    assert_eq!(sequence::fold(&values, 0, |total, value| total + value), 10);
}
