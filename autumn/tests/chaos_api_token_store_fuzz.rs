use autumn_web::auth::{ApiTokenStore, InMemoryApiTokenStore};
use proptest::prelude::*;
use std::collections::HashMap;
use std::future::Future;

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

proptest! {
    #[test]
    fn token_issue_and_verify_fuzz(
        users in proptest::collection::vec(proptest::string::string_regex("[a-zA-Z0-9]+").unwrap(), 1..100)
    ) {
        let store = InMemoryApiTokenStore::default();
        let mut tokens = HashMap::new();

        for user in &users {
            let token = block_on(store.issue(user)).unwrap();
            tokens.insert(token, user.clone());
        }

        for (token, user) in &tokens {
            let v = block_on(store.verify(token)).unwrap();
            assert_eq!(v, Some(user.clone()));
        }

        // Test revoke
        if let Some((token, _)) = tokens.iter().next() {
             block_on(store.revoke(token)).unwrap();
             let v = block_on(store.verify(token)).unwrap();
             assert_eq!(v, None);
        }
    }
}
