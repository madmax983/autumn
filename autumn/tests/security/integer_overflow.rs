use autumn_web::task::parse_duration;

#[test]
fn test_parse_duration_overflow() {
    // These should not panic but return None

    // Overflow in checked_add
    // u64::MAX is 18446744073709551615
    assert_eq!(parse_duration("18446744073709551615s 10s"), None);

    // Overflow in checked_mul
    assert_eq!(parse_duration("18446744073709551615d"), None);
}
