//! Shared helpers for attribute macros that inject hidden extractor
//! parameters (`#[secured]`, `#[authorize]`).
//!
//! Both macros want to add `__autumn_session: Session` and
//! `__autumn_state: State<AppState>` to a handler's argument list.
//! Stacking them is the documented common
//! pattern (`#[secured]` answers "are you in?", `#[authorize]`
//! answers "are you allowed?"), but each macro must avoid double-
//! injecting a parameter that the other already added — duplicate
//! parameter names are a compile error.
//!
//! Both attribute orderings need to work:
//!
//! - `#[secured]` outer / `#[authorize]` inner: `#[secured]` runs
//!   first, injects `__autumn_session` and `__autumn_state`.
//!   `#[authorize]` then runs on the modified function, sees the
//!   existing parameters, and skips re-injection.
//! - `#[authorize]` outer / `#[secured]` inner: `#[authorize]`
//!   runs first, injects `__autumn_session` and `__autumn_state`.
//!   `#[secured]` then runs on the modified function, sees the
//!   existing parameters, and skips re-injection.

use syn::ItemFn;

/// Return `true` when `func` already has a parameter bound to a
/// pattern with the given identifier name.
pub fn has_input_named(func: &ItemFn, name: &str) -> bool {
    func.sig.inputs.iter().any(|arg| match arg {
        syn::FnArg::Typed(pt) => pat_binds_name(&pt.pat, name),
        syn::FnArg::Receiver(_) => false,
    })
}

fn pat_binds_name(pat: &syn::Pat, name: &str) -> bool {
    match pat {
        syn::Pat::Ident(i) => i.ident == name,
        // `State(__autumn_state)`: walk the inner pattern.
        syn::Pat::TupleStruct(ts) => ts.elems.iter().any(|p| pat_binds_name(p, name)),
        syn::Pat::Tuple(t) => t.elems.iter().any(|p| pat_binds_name(p, name)),
        syn::Pat::Struct(s) => s.fields.iter().any(|fp| pat_binds_name(&fp.pat, name)),
        syn::Pat::Reference(r) => pat_binds_name(&r.pat, name),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn detects_simple_ident_binding() {
        let f: ItemFn = parse_quote! {
            async fn h(__autumn_session: Session) {}
        };
        assert!(has_input_named(&f, "__autumn_session"));
        assert!(!has_input_named(&f, "__autumn_state"));
    }

    #[test]
    fn detects_tuple_struct_binding_for_state() {
        let f: ItemFn = parse_quote! {
            async fn h(State(__autumn_state): State<AppState>) {}
        };
        assert!(has_input_named(&f, "__autumn_state"));
        assert!(!has_input_named(&f, "__autumn_session"));
    }

    #[test]
    fn no_inputs_returns_false() {
        let f: ItemFn = parse_quote! { async fn h() {} };
        assert!(!has_input_named(&f, "__autumn_session"));
    }

    #[test]
    fn finds_in_later_position() {
        let f: ItemFn = parse_quote! {
            async fn h(other: i32, __autumn_session: Session) {}
        };
        assert!(has_input_named(&f, "__autumn_session"));
    }
}
