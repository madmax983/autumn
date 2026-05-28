use autumn_web::AppState;
use proptest::prelude::*;

proptest! {
    #[test]
    fn app_state_extensions_fuzz(
        exts1 in proptest::collection::vec(any::<u32>(), 0..10),
        exts2 in proptest::collection::vec(proptest::string::string_regex("[a-z]+").unwrap(), 0..10)
    ) {
        let state = AppState::for_test();
        for ext in exts1 {
            state.insert_extension(ext);
            assert_eq!(state.extension::<u32>().map(|a| *a), Some(ext));
        }
        for ext in exts2 {
            let e = ext.clone();
            state.insert_extension(e);
            assert_eq!(state.extension::<String>().map(|a| a.to_string()), Some(ext));
        }
    }
}
