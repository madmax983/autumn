//! `#[repository(Model)]` proc macro implementation.
//!
//! Generates a concrete `PgXxxRepository` struct with:
//! - Auto-generated CRUD (`find_by_id`, `find_all`, save, update, `delete_by_id`, count, `exists_by_id`)
//! - Derived queries parsed from trait method names (`find_by_field`, `count_by_field`, etc.)
//! - `FromRequestParts` extractor impl

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser as _;
use syn::{Ident, ItemTrait, LitStr, TraitItem};

use crate::model::infer_table_name;

struct RepoConfig {
    model_name: Ident,
    table_name: String,
}

fn parse_repo_args(attr: TokenStream) -> syn::Result<RepoConfig> {
    let mut model_name: Option<Ident> = None;
    let mut table_name: Option<String> = None;

    syn::meta::parser(|meta| {
        if meta.path.get_ident().is_some() && model_name.is_none() && !meta.path.is_ident("table") {
            model_name = Some(meta.path.get_ident().unwrap().clone());
            Ok(())
        } else if meta.path.is_ident("table") {
            let value: LitStr = meta.value()?.parse()?;
            table_name = Some(value.value());
            Ok(())
        } else {
            Err(meta.error("expected model name or table = \"...\""))
        }
    })
    .parse2(attr)?;

    let model = model_name.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "expected model name: #[repository(ModelName)]",
        )
    })?;
    let table = table_name.unwrap_or_else(|| infer_table_name(&model));

    Ok(RepoConfig {
        model_name: model,
        table_name: table,
    })
}

/// Parse a derived query method name like `find_by_title_and_published`.
struct DerivedQuery {
    prefix: String,      // "find", "count", "delete", "exists"
    fields: Vec<String>, // ["title", "published"]
    #[allow(dead_code)] // reserved for Tier 2 OR support
    combinator: String, // "and" or "or"
}

fn parse_query_name(name: &str) -> Option<DerivedQuery> {
    let prefixes = ["find", "count", "delete", "exists"];
    let prefix = prefixes.iter().find(|p| name.starts_with(*p))?;

    let rest = name.strip_prefix(prefix)?;
    let rest = rest.strip_prefix("_by_")?;

    // Split on _and_ or _or_
    let (fields, combinator) = if rest.contains("_and_") {
        if rest.contains("_or_") {
            return None; // Can't mix
        }
        let parts: Vec<String> = rest.split("_and_").map(String::from).collect();
        (parts, "and".to_string())
    } else if rest.contains("_or_") {
        let parts: Vec<String> = rest.split("_or_").map(String::from).collect();
        (parts, "or".to_string())
    } else {
        (vec![rest.to_string()], "and".to_string())
    };

    Some(DerivedQuery {
        prefix: (*prefix).to_string(),
        fields,
        combinator,
    })
}

#[allow(clippy::too_many_lines)]
fn generate_derived_query(
    query: &DerivedQuery,
    table_ident: &Ident,
    model_name: &Ident,
) -> TokenStream {
    let field_idents: Vec<Ident> = query.fields.iter().map(|f| format_ident!("{f}")).collect();
    let param_names: Vec<Ident> = query.fields.iter().map(|f| format_ident!("{f}")).collect();

    // Build filter chain
    let filters: Vec<TokenStream> = field_idents
        .iter()
        .zip(param_names.iter())
        .map(|(field, param)| {
            quote! { .filter(#table_ident::#field.eq(#param)) }
        })
        .collect();

    match query.prefix.as_str() {
        "find" => {
            quote! {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    #table_ident::table
                        #(#filters)*
                        .load::<#model_name>(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }
        }
        "count" => {
            quote! {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    #table_ident::table
                        #(#filters)*
                        .count()
                        .get_result::<i64>(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }
        }
        "delete" => {
            quote! {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    ::autumn_web::reexports::diesel::delete(#table_ident::table #(#filters)*)
                        .execute(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)?;
                Ok(())
            }
        }
        "exists" => {
            quote! {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    ::autumn_web::reexports::diesel::select(::autumn_web::reexports::diesel::dsl::exists(
                        #table_ident::table #(#filters)*
                    )).get_result::<bool>(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }
        }
        _ => quote! { todo!() },
    }
}

#[allow(clippy::too_many_lines)]
pub fn repository_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let config = match parse_repo_args(attr) {
        Ok(c) => c,
        Err(err) => return err.to_compile_error(),
    };

    let trait_def: ItemTrait = match syn::parse2(item) {
        Ok(t) => t,
        Err(err) => return err.to_compile_error(),
    };

    let model_name = &config.model_name;
    let table_name = &config.table_name;
    let table_ident = format_ident!("{table_name}");
    let trait_name = &trait_def.ident;
    let pg_name = format_ident!("Pg{trait_name}");
    let new_name = format_ident!("New{model_name}");
    let update_name = format_ident!("Update{model_name}");
    let vis = &trait_def.vis;

    // Parse derived query methods from trait body
    let mut derived_trait_methods = Vec::new();
    let mut derived_impl_methods = Vec::new();

    for item in &trait_def.items {
        if let TraitItem::Fn(method) = item {
            let method_name = method.sig.ident.to_string();
            if let Some(query) = parse_query_name(&method_name) {
                let fn_ident = &method.sig.ident;

                // Generate parameter list from query fields
                let params: Vec<TokenStream> = query
                    .fields
                    .iter()
                    .map(|f| {
                        let field_ident = format_ident!("{f}");
                        // Use generic owned types for interact closure compatibility
                        quote! { #field_ident: String }
                    })
                    .collect();

                // Determine return type from prefix
                let return_type = match query.prefix.as_str() {
                    "find" => quote! { Vec<#model_name> },
                    "count" => quote! { i64 },
                    "exists" => quote! { bool },
                    _ => quote! { () }, // delete + unknown
                };

                let body = generate_derived_query(&query, &table_ident, model_name);

                derived_trait_methods.push(quote! {
                    async fn #fn_ident(&self, #(#params),*) -> ::autumn_web::AutumnResult<#return_type>;
                });

                derived_impl_methods.push(quote! {
                    async fn #fn_ident(&self, #(#params),*) -> ::autumn_web::AutumnResult<#return_type> {
                        #body
                    }
                });
            }
        }
    }

    quote! {
        /// Generated repository trait with CRUD + derived queries.
        #[::autumn_web::reexports::axum::async_trait]
        #vis trait #trait_name: Send + Sync {
            async fn find_by_id(&self, id: i32) -> ::autumn_web::AutumnResult<Option<#model_name>>;
            async fn find_all(&self) -> ::autumn_web::AutumnResult<Vec<#model_name>>;
            async fn save(&self, new: &#new_name) -> ::autumn_web::AutumnResult<#model_name>;
            async fn update(&self, id: i32, changes: &#update_name) -> ::autumn_web::AutumnResult<#model_name>;
            async fn delete_by_id(&self, id: i32) -> ::autumn_web::AutumnResult<()>;
            async fn count(&self) -> ::autumn_web::AutumnResult<i64>;
            async fn exists_by_id(&self, id: i32) -> ::autumn_web::AutumnResult<bool>;
            #(#derived_trait_methods)*
        }

        /// Postgres implementation of the repository.
        #[derive(Clone)]
        #vis struct #pg_name {
            pool: ::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<::autumn_web::reexports::diesel_async::AsyncPgConnection>,
        }

        #[::autumn_web::reexports::axum::async_trait]
        impl #trait_name for #pg_name {
            async fn find_by_id(&self, id: i32) -> ::autumn_web::AutumnResult<Option<#model_name>> {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    #table_ident::table.find(id).first::<#model_name>(conn).optional()
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }

            async fn find_all(&self) -> ::autumn_web::AutumnResult<Vec<#model_name>> {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(|conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    #table_ident::table.load::<#model_name>(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }

            async fn save(&self, new: &#new_name) -> ::autumn_web::AutumnResult<#model_name> {
                let new = new.clone();
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
                        .values(&new)
                        .get_result::<#model_name>(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }

            async fn update(&self, id: i32, changes: &#update_name) -> ::autumn_web::AutumnResult<#model_name> {
                let changes = changes.clone();
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                        .set(&changes)
                        .get_result::<#model_name>(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }

            async fn delete_by_id(&self, id: i32) -> ::autumn_web::AutumnResult<()> {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    ::autumn_web::reexports::diesel::delete(#table_ident::table.find(id)).execute(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)?;
                Ok(())
            }

            async fn count(&self) -> ::autumn_web::AutumnResult<i64> {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(|conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    #table_ident::table.count().get_result::<i64>(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }

            async fn exists_by_id(&self, id: i32) -> ::autumn_web::AutumnResult<bool> {
                let conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.interact(move |conn| {
                    use ::autumn_web::reexports::diesel::prelude::*;
                    ::autumn_web::reexports::diesel::select(::autumn_web::reexports::diesel::dsl::exists(#table_ident::table.find(id)))
                        .get_result::<bool>(conn)
                }).await.map_err(|e| ::autumn_web::AutumnError::from(::std::io::Error::other(e.to_string())))?
                    .map_err(::autumn_web::AutumnError::from)
            }

            #(#derived_impl_methods)*
        }

        // Extractor: extract from AppState's pool
        #[::autumn_web::reexports::axum::async_trait]
        impl<S> ::autumn_web::reexports::axum::extract::FromRequestParts<S> for #pg_name
        where
            S: Send + Sync,
            ::autumn_web::AppState: ::autumn_web::reexports::axum::extract::FromRef<S>,
        {
            type Rejection = ::autumn_web::AutumnError;

            async fn from_request_parts(
                _parts: &mut ::autumn_web::reexports::http::request::Parts,
                state: &S,
            ) -> Result<Self, Self::Rejection> {
                use ::autumn_web::reexports::axum::extract::FromRef;
                let app_state = ::autumn_web::AppState::from_ref(state);
                let pool = app_state.pool
                    .ok_or_else(|| ::autumn_web::AutumnError::service_unavailable_msg("No database pool configured"))?;
                Ok(#pg_name { pool })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_find_by_single_field() {
        let q = parse_query_name("find_by_title").unwrap();
        assert_eq!(q.prefix, "find");
        assert_eq!(q.fields, vec!["title"]);
    }

    #[test]
    fn parse_find_by_two_fields() {
        let q = parse_query_name("find_by_title_and_published").unwrap();
        assert_eq!(q.prefix, "find");
        assert_eq!(q.fields, vec!["title", "published"]);
        assert_eq!(q.combinator, "and");
    }

    #[test]
    fn parse_count_by() {
        let q = parse_query_name("count_by_published").unwrap();
        assert_eq!(q.prefix, "count");
        assert_eq!(q.fields, vec!["published"]);
    }

    #[test]
    fn parse_delete_by() {
        let q = parse_query_name("delete_by_published").unwrap();
        assert_eq!(q.prefix, "delete");
    }

    #[test]
    fn parse_exists_by() {
        let q = parse_query_name("exists_by_title").unwrap();
        assert_eq!(q.prefix, "exists");
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse_query_name("save").is_none());
        assert!(parse_query_name("custom_method").is_none());
    }

    #[test]
    fn mixed_and_or_returns_none() {
        assert!(parse_query_name("find_by_a_and_b_or_c").is_none());
    }
}
