fn main() {
    let mut value: u64 = 0;
    for index in 1..=50_000_000_u64 {
        value += index;
    }
    println!("{value}");
}
