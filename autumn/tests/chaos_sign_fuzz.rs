use autumn_web::storage::local::sign_upload_legacy;
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_sign_legacy_collision(k1 in ".*", c1 in ".*", e1 in any::<u64>(), k2 in ".*", c2 in ".*", e2 in any::<u64>()) {
        if k1 != k2 || c1 != c2 || e1 != e2 {
            let s1 = sign_upload_legacy(b"secret", &k1, &c1, e1);
            let s2 = sign_upload_legacy(b"secret", &k2, &c2, e2);
            prop_assert_ne!(s1, s2);
        }
    }
}
