use super::*;

fn reference(raw: u64) -> ManagedReference {
    ManagedReference::new(raw)
}

#[test]
fn monotonic_segment_storage_constructs_only_live_values() {
    let mut table = ObjectTable::new();
    table.insert_fresh(reference(1), 10).expect("first");
    table.insert_fresh(reference(2), 20).expect("second");

    let Segment::Dense { first, values } = table.segments[0].as_ref().expect("segment") else {
        panic!("monotonic insertion must stay dense");
    };
    assert_eq!(*first, 0);
    assert_eq!(values.iter().copied().collect::<Vec<_>>(), vec![10, 20]);
}

#[test]
fn segmented_table_preserves_exact_token_order_across_segments() {
    let mut table = ObjectTable::new();
    table.insert(reference(1_024), "fourth");
    table.insert(reference(257), "third");
    table.insert(reference(1), "first");
    table.insert(reference(256), "second");

    assert_eq!(table.len(), 4);
    assert_eq!(
        table
            .iter()
            .map(|(reference, value)| (reference.raw(), *value))
            .collect::<Vec<_>>(),
        vec![
            (1, "first"),
            (256, "second"),
            (257, "third"),
            (1_024, "fourth")
        ]
    );
    assert_eq!(
        table
            .next_after(Some(reference(256)))
            .map(|entry| entry.0.raw()),
        Some(257)
    );
}

#[test]
fn fresh_insertion_accepts_only_a_new_highest_token() {
    let mut table = ObjectTable::new();

    assert_eq!(table.insert_fresh(reference(1), "first"), Ok(()));
    assert_eq!(table.insert_fresh(reference(2), "second"), Ok(()));
    assert_eq!(
        table.insert_fresh(reference(2), "duplicate"),
        Err("duplicate")
    );
    assert_eq!(table.insert_fresh(reference(1), "older"), Err("older"));
    assert_eq!(
        table
            .iter()
            .map(|(reference, value)| (reference.raw(), *value))
            .collect::<Vec<_>>(),
        vec![(1, "first"), (2, "second")]
    );
}

#[test]
fn segmented_table_removes_empty_segments_and_skips_their_token_ranges() {
    let mut table = ObjectTable::new();
    table.insert(reference(1), 10);
    table.insert(reference(256), 20);
    table.insert(reference(257), 30);

    assert_eq!(table.remove(&reference(1)), Some(10));
    assert_eq!(table.remove(&reference(256)), Some(20));
    assert!(!table.contains_key(&reference(1)));
    assert_eq!(table.next_after(None).map(|entry| entry.0.raw()), Some(257));
    assert_eq!(table.remove(&reference(257)), Some(30));
    assert_eq!(table.len(), 0);
    assert!(table.next_after(None).is_none());
}

#[test]
fn dense_batch_insertion_populates_every_segment_entry() {
    let mut table = ObjectTable::new();
    let entries = (1..601_usize).map(|raw| {
        let raw = u64::try_from(raw).expect("test index");
        (reference(raw), raw * 10)
    });
    table
        .insert_reserved_batch(entries)
        .expect("fresh dense batch");

    assert_eq!(table.len(), 600);
    assert!(table.contains_range(reference(1), reference(600)));
    assert_eq!(table.get(&reference(1)), Some(&10));
    assert_eq!(table.get(&reference(256)), Some(&2_560));
    assert_eq!(table.get(&reference(257)), Some(&2_570));
    assert_eq!(table.get(&reference(600)), Some(&6_000));
}

#[test]
fn interior_removal_converts_one_dense_segment_without_losing_neighbors() {
    let mut table = ObjectTable::new();
    table
        .insert_reserved_batch((1..5_usize).map(|raw| {
            let raw = u64::try_from(raw).expect("test index");
            (reference(raw), raw)
        }))
        .expect("dense batch");

    assert_eq!(table.remove(&reference(2)), Some(2));
    assert_eq!(table.get(&reference(1)), Some(&1));
    assert_eq!(table.get(&reference(2)), None);
    assert_eq!(table.get(&reference(3)), Some(&3));
    assert_eq!(table.get(&reference(4)), Some(&4));
    assert_eq!(
        table.iter().map(|entry| entry.0.raw()).collect::<Vec<_>>(),
        vec![1, 3, 4]
    );
}
