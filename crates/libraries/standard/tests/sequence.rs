use pop_standard::sequence;

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
