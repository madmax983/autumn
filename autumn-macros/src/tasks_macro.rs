//! `tasks![]` collection macro.
//!
//! Collects `#[scheduled]`-annotated task handlers into a `Vec<TaskInfo>`,
//! parallel to the `routes![]` macro.

use proc_macro2::TokenStream;

pub fn tasks_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_task_info_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn test_tasks_macro() {
        let input = quote! { handler_a, cron::handler_b };
        let result = tasks_macro(input);
        let result_str = result.to_string();

        assert!(result_str.contains("__autumn_task_info_handler_a"));
        assert!(result_str.contains("cron :: __autumn_task_info_handler_b"));
    }
}
