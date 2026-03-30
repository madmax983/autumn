use proc_macro2::TokenStream;

pub fn islands_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_island_meta_")
}
