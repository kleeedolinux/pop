fn main() {
    let mut total: u64 = 0;
    for index in 1..=20_000_u64 {
        let values = vec![index; 256];
        total += values[0];
        std::hint::black_box(values);
    }
    println!("{total}");
}
