use autumn_web::db::scrub_sql;

#[test]
fn test_scrub_sql_fuzz() {
    proptest::proptest!(|(s in "\\PC*")| {
        let _ = scrub_sql(&s);
    });
}

#[test]
fn test_scrub_sql_overlapping_dollar_quotes() {
    // This case will hang or panic (in previous impls) or incorrectly parse
    // if overlapping matches are not handled correctly.
    let cases = vec![
        ("$tag$$t$tag$", "'?'"),
        ("$tag$$t$$tag$", "'?'"),
        ("$tag$$t$t$tag$", "'?'"),
        ("$tag$$ta$tag$", "'?'"),
        ("$$$$$$$$", "'?''?'"),
        ("$t$$t$t$", "'?'t$"),
        (
            "SELECT * FROM users WHERE note = $tag$$ta$tag$",
            "SELECT * FROM users WHERE note = '?'",
        ),
    ];

    for (input, expected) in cases {
        let scrubbed = scrub_sql(input);
        assert_eq!(scrubbed, expected, "Failed for input: {}", input);
    }
}
