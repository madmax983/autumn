//! `#[autumn_web::main]` macro implementation.
//!
//! Generates a synchronous `main()` that builds a tokio runtime and
//! blocks on the user's async body. We generate the runtime manually
//! instead of delegating to `#[tokio::main]` because `tokio::main`
//! emits code with `::tokio::` paths, which don't resolve when the
//! user only depends on `autumn-web`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ItemFn;

pub fn main_macro(item: TokenStream) -> TokenStream {
    let input_fn: ItemFn = match syn::parse2(item) {
        Ok(f) => f,
        Err(err) => return err.to_compile_error(),
    };

    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(input_fn.sig.fn_token, "the main function must be async")
            .to_compile_error();
    }

    let body = &input_fn.block;
    let attrs = &input_fn.attrs;

    quote! {
        #(#attrs)*
        fn main() {
            // Tell the framework where autumn.toml lives (the app's crate root).
            // SAFETY: called at the top of main, before any threads are spawned.
            unsafe { ::std::env::set_var("AUTUMN_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR")); }

            // Tell the framework whether the *user's* crate was built in debug mode.
            // cfg!(debug_assertions) evaluates here — in the user's crate context —
            // so it reflects their build mode, not autumn-web's library build mode.
            // SAFETY: called at the top of main, before any threads are spawned.
            unsafe {
                ::std::env::set_var(
                    "AUTUMN_IS_DEBUG",
                    if cfg!(debug_assertions) { "1" } else { "0" },
                );
            }

            ::autumn_web::reexports::tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime")
                .block_on(async move #body);
        }
    }
}
