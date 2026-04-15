//! `#[repository(Model)]` proc macro implementation.
//!
//! Generates a concrete `PgXxxRepository` struct with:
//! - Auto-generated CRUD (`find_by_id`, `find_all`, save, update, `delete_by_id`, count, `exists_by_id`)
//! - Derived queries parsed from trait method names (`find_by_field`, `count_by_field`, etc.)
//! - `FromRequestParts` extractor impl
//!
//! Uses native async fn in traits (Rust 1.75+) — no `async_trait` crate needed.
//! Uses `diesel-async` `RunQueryDsl` for async queries — no sync `interact()`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser as _;
use syn::{Ident, ItemTrait, LitStr, TraitItem};

use crate::model::infer_table_name;

fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

struct RepoConfig {
    model_name: Ident,
    table_name: String,
    hooks_type: Option<Ident>,
    api_path: Option<String>,
}

fn parse_repo_args(attr: TokenStream) -> syn::Result<RepoConfig> {
    let mut model_name: Option<Ident> = None;
    let mut table_name: Option<String> = None;
    let mut hooks_type: Option<Ident> = None;
    let mut api_path: Option<String> = None;

    syn::meta::parser(|meta| {
        // `hooks = Ident` must be checked before the catch-all model_name case,
        // otherwise "hooks" would be parsed as the model name.
        if meta.path.is_ident("hooks") {
            let value: Ident = meta.value()?.parse()?;
            hooks_type = Some(value);
            Ok(())
        } else if meta.path.is_ident("table") {
            let value: LitStr = meta.value()?.parse()?;
            table_name = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("api") {
            let value: LitStr = meta.value()?.parse()?;
            api_path = Some(value.value());
            Ok(())
        } else if meta.path.get_ident().is_some() && model_name.is_none() {
            model_name = Some(meta.path.get_ident().unwrap().clone());
            Ok(())
        } else {
            Err(meta
                .error("expected model name, table = \"...\", hooks = Type, or api = \"/path\""))
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
        hooks_type,
        api_path,
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
            quote! { .filter(#table_ident::#field.eq(&#param)) }
        })
        .collect();

    match query.prefix.as_str() {
        "find" => {
            quote! {
                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                #table_ident::table
                    #(#filters)*
                    .load::<#model_name>(&mut conn)
                    .await
                    .map_err(::autumn_web::AutumnError::from)
            }
        }
        "count" => {
            quote! {
                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                #table_ident::table
                    #(#filters)*
                    .count()
                    .get_result::<i64>(&mut conn)
                    .await
                    .map_err(::autumn_web::AutumnError::from)
            }
        }
        "delete" => {
            quote! {
                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                ::autumn_web::reexports::diesel::delete(#table_ident::table #(#filters)*)
                    .execute(&mut conn)
                    .await
                    .map_err(::autumn_web::AutumnError::from)?;
                Ok(())
            }
        }
        "exists" => {
            quote! {
                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                ::autumn_web::reexports::diesel::select(
                    ::autumn_web::reexports::diesel::dsl::exists(
                        #table_ident::table #(#filters)*
                    )
                )
                .get_result::<bool>(&mut conn)
                .await
                .map_err(::autumn_web::AutumnError::from)
            }
        }
        _ => {
            let msg = format!(
                "Unsupported query prefix: {}. Supported prefixes are find, count, delete, exists.",
                query.prefix
            );
            quote! { ::core::compile_error!(#msg); }
        }
    }
}

#[allow(clippy::too_many_lines, clippy::option_if_let_else)]
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

                // Use the user's actual parameter types from the trait signature.
                // The user writes: fn find_by_tag(tag: String) -> Vec<Bookmark>
                // We extract the (name: Type) pairs directly.
                let user_params: Vec<TokenStream> = method
                    .sig
                    .inputs
                    .iter()
                    .filter_map(|arg| {
                        if let syn::FnArg::Typed(pat_type) = arg {
                            let pat = &pat_type.pat;
                            let ty = &pat_type.ty;
                            Some(quote! { #pat: #ty })
                        } else {
                            None // skip `self`
                        }
                    })
                    .collect();

                // Determine return type from prefix
                let return_type = match query.prefix.as_str() {
                    "find" => quote! { Vec<#model_name> },
                    "count" => quote! { i64 },
                    "exists" => quote! { bool },
                    _ => quote! { () }, // delete + unknown
                };

                let params = &user_params;

                let body = generate_derived_query(&query, &table_ident, model_name);

                derived_trait_methods.push(quote! {
                    fn #fn_ident(&self, #(#params),*) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<#return_type>> + Send;
                });

                derived_impl_methods.push(quote! {
                    async fn #fn_ident(&self, #(#params),*) -> ::autumn_web::AutumnResult<#return_type> {
                        use ::autumn_web::reexports::diesel::prelude::*;
                        use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                        #body
                    }
                });
            }
        }
    }

    // ── Build struct fields, extractor init, and CRUD bodies ──────────────
    //
    // When `hooks_type` is present, the struct gains a `hooks` field,
    // the extractor initialises it with `Default::default()`, and the
    // save / update / delete methods are wrapped in a transactional
    // hook lifecycle (before_* → persist).
    //
    // When absent, the generated code is identical to the pre-hooks version
    // (zero-cost path).

    let (struct_fields, extractor_init, save_body, update_body, delete_body) = if let Some(
        ref hooks_ident,
    ) =
        config.hooks_type
    {
        // ── Struct fields with hooks ───────────────────────
        let struct_fields = quote! {
            pool: ::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<
                ::autumn_web::reexports::diesel_async::AsyncPgConnection,
            >,
            hooks: #hooks_ident,
        };

        let extractor_init = quote! {
            Ok(#pg_name {
                pool,
                hooks: <#hooks_ident as ::std::default::Default>::default(),
            })
        };

        // ── save (hooked) ─────────────────────────────────
        let save_body = quote! {
            use ::autumn_web::reexports::diesel::prelude::*;
            use ::autumn_web::reexports::diesel_async::RunQueryDsl;
            use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            let mut input = new.clone();
            let mut ctx = MutationContext::new(MutationOp::Create);

            // before_create can validate/reject/rewrite
            self.hooks.before_create(&mut ctx, &mut input).await?;

            let record = ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
                .values(&input)
                .get_result::<#model_name>(&mut conn)
                .await
                .map_err(::autumn_web::AutumnError::from)?;

            Ok(record)
        };

        // ── update (hooked) ───────────────────────────────
        let draft_ext_trait = format_ident!("{}DraftExt", model_name);
        let update_body = quote! {
            use ::autumn_web::reexports::diesel::prelude::*;
            use ::autumn_web::reexports::diesel_async::RunQueryDsl;
            use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks, UpdateDraft};

            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            let mut ctx = MutationContext::new(MutationOp::Update);

            // Load current record
            let current = #table_ident::table
                .find(id)
                .first::<#model_name>(&mut conn)
                .await
                .optional()
                .map_err(::autumn_web::AutumnError::from)?
                .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                    format!("{} with id {} not found", stringify!(#model_name), id)
                ))?;

            // Build merged draft from current + patch
            let mut draft = <UpdateDraft<#model_name> as #draft_ext_trait>::from_patch(&current, changes)?;

            // before_update can inspect/rewrite via draft field accessors
            self.hooks.before_update(&mut ctx, &mut draft).await?;

            // Persist the proposed state
            let proposed = draft.into_after();
            let record = ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                .set(&proposed)
                .get_result::<#model_name>(&mut conn)
                .await
                .map_err(::autumn_web::AutumnError::from)?;

            Ok(record)
        };

        // ── delete (hooked) ───────────────────────────────
        let delete_body = quote! {
            use ::autumn_web::reexports::diesel::prelude::*;
            use ::autumn_web::reexports::diesel_async::RunQueryDsl;
            use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            let mut ctx = MutationContext::new(MutationOp::Delete);

            // Load current record for before_delete context
            let record = #table_ident::table
                .find(id)
                .first::<#model_name>(&mut conn)
                .await
                .optional()
                .map_err(::autumn_web::AutumnError::from)?
                .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                    format!("{} with id {} not found", stringify!(#model_name), id)
                ))?;

            self.hooks.before_delete(&mut ctx, &record).await?;

            ::autumn_web::reexports::diesel::delete(#table_ident::table.find(id))
                .execute(&mut conn)
                .await
                .map_err(::autumn_web::AutumnError::from)?;

            Ok(())
        };

        (
            struct_fields,
            extractor_init,
            save_body,
            update_body,
            delete_body,
        )
    } else {
        // ── No hooks: existing zero-cost path ─────────────

        let struct_fields = quote! {
            pool: ::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<
                ::autumn_web::reexports::diesel_async::AsyncPgConnection,
            >,
        };

        let extractor_init = quote! {
            Ok(#pg_name { pool })
        };

        let save_body = quote! {
            use ::autumn_web::reexports::diesel::prelude::*;
            use ::autumn_web::reexports::diesel_async::RunQueryDsl;
            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
                .values(new)
                .get_result::<#model_name>(&mut conn)
                .await
                .map_err(::autumn_web::AutumnError::from)
        };

        let update_body = quote! {
            use ::autumn_web::reexports::diesel::prelude::*;
            use ::autumn_web::reexports::diesel_async::RunQueryDsl;
            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            let diesel_changeset = changes.__to_changeset();
            ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                .set(&diesel_changeset)
                .get_result::<#model_name>(&mut conn)
                .await
                .map_err(::autumn_web::AutumnError::from)
        };

        let delete_body = quote! {
            use ::autumn_web::reexports::diesel::prelude::*;
            use ::autumn_web::reexports::diesel_async::RunQueryDsl;
            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            ::autumn_web::reexports::diesel::delete(#table_ident::table.find(id))
                .execute(&mut conn)
                .await
                .map_err(::autumn_web::AutumnError::from)?;
            Ok(())
        };

        (
            struct_fields,
            extractor_init,
            save_body,
            update_body,
            delete_body,
        )
    };

    // ── Build API handlers (when `api = "/path"` is present) ────────────
    let api_handlers = if let Some(ref api_path) = config.api_path {
        let prefix = to_snake_case(&model_name.to_string());

        let list_fn = format_ident!("{prefix}_api_list");
        let get_fn = format_ident!("{prefix}_api_get");
        let create_fn = format_ident!("{prefix}_api_create");
        let update_fn = format_ident!("{prefix}_api_update");
        let delete_fn = format_ident!("{prefix}_api_delete");

        let list_info = format_ident!("__autumn_route_info_{prefix}_api_list");
        let get_info = format_ident!("__autumn_route_info_{prefix}_api_get");
        let create_info = format_ident!("__autumn_route_info_{prefix}_api_create");
        let update_info = format_ident!("__autumn_route_info_{prefix}_api_update");
        let delete_info = format_ident!("__autumn_route_info_{prefix}_api_delete");

        let id_path = format!("{api_path}/{{id}}");

        quote! {
            // ── Auto-generated REST API handlers ─────────────────

            #vis async fn #list_fn(
                repo: #pg_name,
            ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<Vec<#model_name>>> {
                Ok(::autumn_web::prelude::Json(repo.find_all().await?))
            }

            #[doc(hidden)]
            #vis fn #list_info() -> ::autumn_web::Route {
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::GET,
                    path: #api_path,
                    handler: ::autumn_web::reexports::axum::routing::get(#list_fn),
                    name: ::core::stringify!(#list_fn),
                }
            }

            #vis async fn #get_fn(
                ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
                repo: #pg_name,
            ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<#model_name>> {
                let record = repo.find_by_id(id).await?
                    .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg("not found"))?;
                Ok(::autumn_web::prelude::Json(record))
            }

            #[doc(hidden)]
            #vis fn #get_info() -> ::autumn_web::Route {
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::GET,
                    path: #id_path,
                    handler: ::autumn_web::reexports::axum::routing::get(#get_fn),
                    name: ::core::stringify!(#get_fn),
                }
            }

            #vis async fn #create_fn(
                repo: #pg_name,
                ::autumn_web::prelude::Json(new): ::autumn_web::prelude::Json<#new_name>,
            ) -> ::autumn_web::AutumnResult<(::autumn_web::reexports::http::StatusCode, ::autumn_web::prelude::Json<#model_name>)> {
                let record = repo.save(&new).await?;
                Ok((::autumn_web::reexports::http::StatusCode::CREATED, ::autumn_web::prelude::Json(record)))
            }

            #[doc(hidden)]
            #vis fn #create_info() -> ::autumn_web::Route {
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::POST,
                    path: #api_path,
                    handler: ::autumn_web::reexports::axum::routing::post(#create_fn),
                    name: ::core::stringify!(#create_fn),
                }
            }

            #vis async fn #update_fn(
                ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
                repo: #pg_name,
                ::autumn_web::prelude::Json(patch): ::autumn_web::prelude::Json<#update_name>,
            ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<#model_name>> {
                let record = repo.update(id, &patch).await?;
                Ok(::autumn_web::prelude::Json(record))
            }

            #[doc(hidden)]
            #vis fn #update_info() -> ::autumn_web::Route {
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::PUT,
                    path: #id_path,
                    handler: ::autumn_web::reexports::axum::routing::put(#update_fn),
                    name: ::core::stringify!(#update_fn),
                }
            }

            #vis async fn #delete_fn(
                ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
                repo: #pg_name,
            ) -> ::autumn_web::AutumnResult<::autumn_web::reexports::http::StatusCode> {
                repo.delete_by_id(id).await?;
                Ok(::autumn_web::reexports::http::StatusCode::NO_CONTENT)
            }

            #[doc(hidden)]
            #vis fn #delete_info() -> ::autumn_web::Route {
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::DELETE,
                    path: #id_path,
                    handler: ::autumn_web::reexports::axum::routing::delete(#delete_fn),
                    name: ::core::stringify!(#delete_fn),
                }
            }
        }
    } else {
        quote! {}
    };

    // Generate the trait, impl, and extractor.
    //
    // Key design decisions:
    // - Native async fn (no #[async_trait]) — Rust 1.75+ supports this
    // - Trait methods use `-> impl Future` for object safety with Send bound
    // - Uses diesel-async RunQueryDsl for async .load()/.first() etc.
    // - Table/New/Update types must be in scope where the macro is invoked
    //   (the user brings them in via `use crate::models::*` or similar)
    quote! {
        /// Generated repository trait with CRUD + derived queries.
        #vis trait #trait_name: Send + Sync {
            fn find_by_id(&self, id: i64) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<Option<#model_name>>> + Send;
            fn find_all(&self) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<Vec<#model_name>>> + Send;
            fn save(&self, new: &#new_name) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<#model_name>> + Send;
            fn update(&self, id: i64, changes: &#update_name) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<#model_name>> + Send;
            fn delete_by_id(&self, id: i64) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<()>> + Send;
            fn count(&self) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<i64>> + Send;
            fn exists_by_id(&self, id: i64) -> impl ::std::future::Future<Output = ::autumn_web::AutumnResult<bool>> + Send;
            #(#derived_trait_methods)*
        }

        /// Postgres implementation of the repository.
        #[derive(Clone)]
        #vis struct #pg_name {
            #struct_fields
        }

        impl #trait_name for #pg_name {
            async fn find_by_id(&self, id: i64) -> ::autumn_web::AutumnResult<Option<#model_name>> {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                #table_ident::table
                    .find(id)
                    .first::<#model_name>(&mut conn)
                    .await
                    .optional()
                    .map_err(::autumn_web::AutumnError::from)
            }

            async fn find_all(&self) -> ::autumn_web::AutumnResult<Vec<#model_name>> {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                #table_ident::table
                    .load::<#model_name>(&mut conn)
                    .await
                    .map_err(::autumn_web::AutumnError::from)
            }

            async fn save(&self, new: &#new_name) -> ::autumn_web::AutumnResult<#model_name> {
                #save_body
            }

            async fn update(&self, id: i64, changes: &#update_name) -> ::autumn_web::AutumnResult<#model_name> {
                #update_body
            }

            async fn delete_by_id(&self, id: i64) -> ::autumn_web::AutumnResult<()> {
                #delete_body
            }

            async fn count(&self) -> ::autumn_web::AutumnResult<i64> {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                #table_ident::table
                    .count()
                    .get_result::<i64>(&mut conn)
                    .await
                    .map_err(::autumn_web::AutumnError::from)
            }

            async fn exists_by_id(&self, id: i64) -> ::autumn_web::AutumnResult<bool> {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                ::autumn_web::reexports::diesel::select(
                    ::autumn_web::reexports::diesel::dsl::exists(#table_ident::table.find(id))
                )
                .get_result::<bool>(&mut conn)
                .await
                .map_err(::autumn_web::AutumnError::from)
            }

            #(#derived_impl_methods)*
        }

        // Extractor: pull pool from AppState (same pattern as Db extractor)
        impl ::autumn_web::reexports::axum::extract::FromRequestParts<::autumn_web::AppState> for #pg_name {
            type Rejection = ::autumn_web::AutumnError;

            async fn from_request_parts(
                _parts: &mut ::autumn_web::reexports::http::request::Parts,
                state: &::autumn_web::AppState,
            ) -> Result<Self, Self::Rejection> {
                let pool = state.pool()
                    .ok_or_else(|| ::autumn_web::AutumnError::service_unavailable_msg("No database pool configured"))?
                    .clone();
                #extractor_init
            }
        }

        #api_handlers
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

    #[test]
    fn parse_repo_args_with_hooks() {
        let tokens: proc_macro2::TokenStream = "Post, hooks = PostHooks".parse().unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert_eq!(config.model_name.to_string(), "Post");
        assert_eq!(
            config
                .hooks_type
                .as_ref()
                .map(std::string::ToString::to_string),
            Some("PostHooks".to_string())
        );
    }

    #[test]
    fn parse_repo_args_without_hooks() {
        let tokens: proc_macro2::TokenStream = "Post".parse().unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert_eq!(config.model_name.to_string(), "Post");
        assert!(config.hooks_type.is_none());
    }

    #[test]
    fn parse_repo_args_with_table_and_hooks() {
        let tokens: proc_macro2::TokenStream =
            r#"Post, table = "blog_posts", hooks = PostHooks"#.parse().unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert_eq!(config.model_name.to_string(), "Post");
        assert_eq!(config.table_name, "blog_posts");
        assert_eq!(
            config
                .hooks_type
                .as_ref()
                .map(std::string::ToString::to_string),
            Some("PostHooks".to_string())
        );
    }

    #[test]
    fn parse_repo_args_with_api() {
        let tokens: proc_macro2::TokenStream = r#"Post, api = "/api/posts""#.parse().unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert_eq!(config.model_name.to_string(), "Post");
        assert_eq!(config.api_path.as_deref(), Some("/api/posts"));
    }

    #[test]
    fn parse_repo_args_with_hooks_and_api() {
        let tokens: proc_macro2::TokenStream =
            r#"Post, hooks = PostHooks, api = "/api/v1/posts""#.parse().unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert_eq!(config.model_name.to_string(), "Post");
        assert!(config.hooks_type.is_some());
        assert_eq!(config.api_path.as_deref(), Some("/api/v1/posts"));
    }

    #[test]
    fn parse_repo_args_without_api() {
        let tokens: proc_macro2::TokenStream = "Post".parse().unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert!(config.api_path.is_none());
    }

    #[test]
    fn snake_case_simple() {
        assert_eq!(to_snake_case("Bookmark"), "bookmark");
    }

    #[test]
    fn snake_case_multi_word() {
        assert_eq!(to_snake_case("PageRevision"), "page_revision");
    }

    #[test]
    fn snake_case_already_lower() {
        assert_eq!(to_snake_case("widget"), "widget");
    }
}
