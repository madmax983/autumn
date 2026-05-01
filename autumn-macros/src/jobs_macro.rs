//! `jobs![]` collection macro.

use proc_macro2::TokenStream;

pub fn jobs_macro(input: TokenStream) -> TokenStream {
    crate::collect::collect_companions(input, "__autumn_job_info_")
}
