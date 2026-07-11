use pop_foundation::TextSize;
use pop_source::SourceDatabase;

#[test]
fn file_ids_follow_deterministic_insertion_order() {
    let mut sources = SourceDatabase::new();
    let first = sources
        .add("src/first.pop", "namespace First\n")
        .expect("small source");
    let second = sources
        .add("src/second.pop", "namespace Second\n")
        .expect("small source");

    assert_eq!(first.raw(), 0);
    assert_eq!(second.raw(), 1);
    assert_eq!(
        sources.file(first).expect("first file").path(),
        "src/first.pop"
    );
}

#[test]
fn line_columns_count_unicode_scalars_while_spans_use_bytes() {
    let mut sources = SourceDatabase::new();
    let text = "namespace Café\npublic const NAME = \"Ana\"\n";
    let file = sources.add("src/main.pop", text).expect("small source");
    let source = sources.file(file).expect("source file");
    let end_of_first_line = text.find('\n').expect("newline");

    let position = source
        .line_column(TextSize::try_from_usize(end_of_first_line).expect("small offset"))
        .expect("character boundary");
    assert_eq!((position.line(), position.column()), (0, 14));
    assert_eq!(source.offset(position), Some(TextSize::from_u32(15)));
}
