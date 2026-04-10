fn test_opt2() {
    let due_timers = vec![1, 2, 3];
    let timer_row_ids = due_timers.iter().map(|&x| x).collect::<Vec<_>>();
    println!("{:?}", timer_row_ids);
}
fn main() {
    test_opt2();
}
