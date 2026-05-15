fn main() {
    let header = "https, http";
    let split: Vec<_> = header.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    println!("{:?}", split);
}
