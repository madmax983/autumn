//! `tasks![]` collection macro.
//!
//! Collects `#[scheduled]`-annotated task handlers into a `Vec<TaskInfo>`,
//! parallel to the `routes![]` macro.

use proc_macro2::TokenStream;

pub fn tasks_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_task_info_")
}
