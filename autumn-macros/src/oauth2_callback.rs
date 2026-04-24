use proc_macro2::TokenStream;

use crate::route;

pub fn oauth2_callback_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_macro("GET", "get", attr, item)
}
