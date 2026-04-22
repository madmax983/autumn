use autumn_web::config::AutumnConfig;
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_config_fuzzing(s in "\\PC*") {
        let _ = toml::from_str::<AutumnConfig>(&s);
    }
}
