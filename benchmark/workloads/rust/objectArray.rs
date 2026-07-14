struct Item {
    value: u64,
}

fn main() {
    let mut values = Vec::with_capacity(200_000);
    for index in 1..=200_000_u64 {
        values.push(Box::new(Item { value: index }));
    }
    let total: u64 = values.iter().map(|item| item.value).sum();
    println!("{total}");
}
