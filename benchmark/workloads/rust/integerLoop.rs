fn main() {
    let mut index: i64 = 1;
    let mut value: i64 = 0;

    while index <= 50_000_000 {
        value += index;
        index += 1;
    }

    println!("{value}");
}