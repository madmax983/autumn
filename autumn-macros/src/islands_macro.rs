use proc_macro2::TokenStream;

pub fn islands_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_island_meta_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn collects_island_metadata_companions() {
        let out = islands_macro(quote!(counter, todos)).to_string();

        assert!(out.contains("__autumn_island_meta_counter"));
        assert!(out.contains("__autumn_island_meta_todos"));
    }
}
