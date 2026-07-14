fn main() {
    let mut values = vec![0_u64; 1_000_000];
    for (index, value) in values.iter_mut().enumerate() {
        *value = index as u64 + 1;
    }
    let total: u64 = values.iter().sum();
    println!("{total}");
}
