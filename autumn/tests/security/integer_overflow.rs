use autumn_web::task::parse_duration;

#[test]
fn test_parse_duration_integer_overflow() {
    // Attempting to parse a duration that overflows u64 calculation
    let max_u64 = u64::MAX;
    let malicious_input = format!("{}d", max_u64);

    // This should gracefully return None rather than panicking due to integer overflow
    let result = parse_duration(&malicious_input);
    assert_eq!(result, None);

    let malicious_input2 = format!("{}h", max_u64);
    let result2 = parse_duration(&malicious_input2);
    assert_eq!(result2, None);

    let malicious_input3 = format!("{}m", max_u64);
    let result3 = parse_duration(&malicious_input3);
    assert_eq!(result3, None);

    // Also test overflow on addition
    let half_max = u64::MAX / 2 + 10;
    let malicious_input4 = format!("{}s {}s", half_max, half_max);
    let result4 = parse_duration(&malicious_input4);
    assert_eq!(result4, None);
}
