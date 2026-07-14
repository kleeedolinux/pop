fn fibonacci(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    fibonacci(value - 1) + fibonacci(value - 2)
}

fn main() {
    let mut total = 0;
    for _ in 0..30 {
        total += fibonacci(28);
    }
    println!("{total}");
}
