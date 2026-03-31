use proc_macro2::TokenStream;

pub fn actions_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_action_meta_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn collects_action_metadata_companions() {
        let out = actions_macro(quote!(rename_todo, complete_todo)).to_string();

        assert!(out.contains("__autumn_action_meta_rename_todo"));
        assert!(out.contains("__autumn_action_meta_complete_todo"));
    }
}
