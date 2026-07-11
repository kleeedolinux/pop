use pop_foundation::{FileId, SourceSpan, TextRange, TextSize, WorkspaceId, stable_hash_bytes};

#[test]
fn typed_ids_do_not_erase_ownership_domains() {
    let file = FileId::from_raw(7);
    let workspace = WorkspaceId::from_raw(7);

    assert_eq!(file.raw(), 7);
    assert_eq!(workspace.raw(), 7);
    assert_ne!(format!("{file:?}"), format!("{workspace:?}"));
}

#[test]
fn source_ranges_are_half_open_and_validated() {
    let range =
        TextRange::new(TextSize::from_u32(4), TextSize::from_u32(9)).expect("ordered range");
    let span = SourceSpan::new(FileId::from_raw(2), range);

    assert_eq!(range.len().to_u32(), 5);
    assert_eq!(span.file(), FileId::from_raw(2));
    assert!(TextRange::new(TextSize::from_u32(9), TextSize::from_u32(4)).is_none());
}

#[test]
fn stable_hashing_is_repeatable_and_has_a_fixed_baseline() {
    assert_eq!(
        stable_hash_bytes(b"Pop Lang"),
        stable_hash_bytes(b"Pop Lang")
    );
    assert_eq!(stable_hash_bytes(b"Pop Lang"), 0x32f4_01e0_c020_db3c);
}
