//! `one_off_tasks![]` collection macro.

use proc_macro2::TokenStream;

pub fn one_off_tasks_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_one_off_task_info_")
}
