use autumn_web::task::parse_duration;

#[test]
fn test_parse_duration_integer_overflow() {
    // A string that would previously cause an integer overflow panic
    // when calculating total_secs (9999999999999999999 * 86400)
    let s = "9999999999999999999d";

    // It should safely return None now, instead of panicking.
    assert!(parse_duration(s).is_none());
}
