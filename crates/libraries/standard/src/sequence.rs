pub fn map<T, U>(values: &[T], mut transform: impl FnMut(&T) -> U) -> Vec<U> {
    values.iter().map(&mut transform).collect()
}

pub fn filter<T: Clone>(values: &[T], mut predicate: impl FnMut(&T) -> bool) -> Vec<T> {
    values
        .iter()
        .filter(|value| predicate(value))
        .cloned()
        .collect()
}

pub fn fold<T, State>(
    values: &[T],
    initial: State,
    mut combine: impl FnMut(State, &T) -> State,
) -> State {
    values.iter().fold(initial, &mut combine)
}
