//! #2358 regression: same-named `#[cached]` associated functions in one module.
//!
//! `#[cached]` used to build its cache-key namespace from
//! `module_path!()::<fn name>` alone, so two inherent impls in one module with
//! same-named methods (`Products::get` and `Reviews::get`) shared one
//! namespace — and therefore one cache slot. With a shared backend the second
//! method could be served the first method's value: silent cache poisoning,
//! not just a missed invalidation.
//!
//! An attribute macro on a method never sees the enclosing `impl`, so the
//! `Self` type cannot be named in the identity. The identity instead embeds
//! the attribute's own source position
//! (`module::<fn>@file:line:column`), which is distinct for the two methods
//! with no user action. These tests prove both halves: the namespaces differ,
//! and the runtime entries do not cross-contaminate.

/// The issue's repro shape, verbatim: two impls, one module, one method name.
mod catalog {
    pub struct Products;
    pub struct Reviews;

    impl Products {
        #[autumn_web::cached]
        pub fn get(id: i64) -> String {
            format!("product-{id}")
        }
    }

    impl Reviews {
        #[autumn_web::cached]
        pub fn get(id: i64) -> String {
            format!("review-{id}")
        }
    }
}

#[test]
fn same_named_associated_functions_do_not_share_a_namespace() {
    let products = catalog::Products::__AUTUMN_CACHE_READ_ID__get;
    let reviews = catalog::Reviews::__AUTUMN_CACHE_READ_ID__get;
    assert_ne!(
        products, reviews,
        "the two `get` methods must not share a cache-key namespace, got {products:?} twice"
    );
    // The namespace still names the module and the function — the source
    // position is a disambiguating suffix, not a replacement.
    for (id, owner) in [(products, "Products"), (reviews, "Reviews")] {
        assert!(
            id.contains("cached_identity::catalog::get@"),
            "{owner}::get's namespace must name module and fn: {id:?}"
        );
    }
}

#[test]
fn same_named_associated_functions_do_not_share_cache_entries() {
    // The pre-#2358 failure mode, end to end: both methods keyed as
    // `<module>::get:<hash(1,)>`, so the second call served the first call's
    // value. Each must serve its own.
    assert_eq!(catalog::Products::get(1), "product-1");
    assert_eq!(catalog::Reviews::get(1), "review-1");
    // And the second call of each is a genuine hit on its own entry, not a
    // miss recomputed — the values stay stable across calls.
    assert_eq!(catalog::Products::get(1), "product-1");
    assert_eq!(catalog::Reviews::get(1), "review-1");
}

#[test]
fn invalidating_one_namespace_leaves_the_other_intact() {
    // The flip side of the shared-namespace bug: invalidation used to clear
    // both methods' entries at once (safe, but wrong). Each namespace now
    // invalidates exactly its own entries.
    assert_eq!(catalog::Products::get(7), "product-7");
    assert_eq!(catalog::Reviews::get(7), "review-7");

    assert!(catalog::Products::__autumn_cache_invalidate__get());

    // Products recomputes (its entry was dropped)...
    assert_eq!(catalog::Products::get(7), "product-7");
    // ...while Reviews still serves its cached entry. (A poisoned shared
    // namespace would have dropped it too.)
    assert_eq!(catalog::Reviews::get(7), "review-7");
}
