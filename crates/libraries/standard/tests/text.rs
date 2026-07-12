use pop_standard::text;

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
