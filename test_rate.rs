fn main() {
    let secs = 4.49e307_f64;
    let clamped = secs.clamp(1.0, u64::MAX as f64) as u64;
    println!("{}", clamped);
}
