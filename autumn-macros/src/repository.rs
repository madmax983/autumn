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
    commit_hooks: bool,
    api_path: Option<String>,
    policy_type: Option<Ident>,
    scope_type: Option<Ident>,
}

fn parse_repo_args(attr: TokenStream) -> syn::Result<RepoConfig> {
    let mut model_name: Option<Ident> = None;
    let mut table_name: Option<String> = None;
    let mut hooks_type: Option<Ident> = None;
    let mut commit_hooks = false;
    let mut api_path: Option<String> = None;
    let mut policy_type: Option<Ident> = None;
    let mut scope_type: Option<Ident> = None;

    syn::meta::parser(|meta| {
        // `hooks = Ident` must be checked before the catch-all model_name case,
        // otherwise "hooks" would be parsed as the model name.
        if meta.path.is_ident("hooks") {
            let value: Ident = meta.value()?.parse()?;
            hooks_type = Some(value);
            Ok(())
        } else if meta.path.is_ident("commit_hooks") {
            let value: syn::LitBool = meta.value()?.parse()?;
            commit_hooks = value.value;
            Ok(())
        } else if meta.path.is_ident("table") {
            let value: LitStr = meta.value()?.parse()?;
            table_name = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("api") {
            let value: LitStr = meta.value()?.parse()?;
            api_path = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("policy") {
            let value: Ident = meta.value()?.parse()?;
            policy_type = Some(value);
            Ok(())
        } else if meta.path.is_ident("scope") {
            let value: Ident = meta.value()?.parse()?;
            scope_type = Some(value);
            Ok(())
        } else if meta.path.get_ident().is_some() && model_name.is_none() {
            model_name = Some(meta.path.get_ident().unwrap().clone());
            Ok(())
        } else {
            Err(meta.error(
                "expected model name, table = \"...\", hooks = Type, commit_hooks = true, api = \"/path\", policy = Type, or scope = Type",
            ))
        }
    })
    .parse2(attr)?;

    let model = model_name.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "expected model name: #[repository(ModelName)]",
        )
    })?;
    if commit_hooks && hooks_type.is_none() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "commit_hooks = true requires hooks = Type",
        ));
    }
    let table = table_name.unwrap_or_else(|| infer_table_name(&model));

    Ok(RepoConfig {
        model_name: model,
        table_name: table,
        hooks_type,
        commit_hooks,
        api_path,
        policy_type,
        scope_type,
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
#[allow(clippy::cognitive_complexity)]
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
    let commit_hooks_enabled = config.hooks_type.is_some() && config.commit_hooks;

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

    let (
        struct_fields,
        clone_impl,
        extractor_init,
        save_body,
        update_body,
        delete_body,
        hook_support_methods,
        hook_inventory_registration,
    ) = if let Some(ref hooks_ident) = config.hooks_type {
        // ── Struct fields with hooks ───────────────────────
        let idempotency_struct_field = if commit_hooks_enabled {
            quote! {
                idempotency: ::core::option::Option<::autumn_web::idempotency::IdempotencyContext>,
            }
        } else {
            quote! {}
        };
        let idempotency_clone_field = if commit_hooks_enabled {
            quote! {
                idempotency: self.idempotency.clone(),
            }
        } else {
            quote! {}
        };

        let struct_fields = quote! {
            pool: ::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<
                ::autumn_web::reexports::diesel_async::AsyncPgConnection,
            >,
            hooks: #hooks_ident,
            #idempotency_struct_field
        };

        let clone_impl = quote! {
            impl ::core::clone::Clone for #pg_name {
                fn clone(&self) -> Self {
                    Self {
                        pool: self.pool.clone(),
                        hooks: <#hooks_ident as ::autumn_web::hooks::RepositoryHooksClone>::autumn_clone(&self.hooks),
                        #idempotency_clone_field
                    }
                }
            }
        };

        let extractor_init = if commit_hooks_enabled {
            quote! {
                #pg_name::__autumn_register_repository_commit_hooks();
                Ok(#pg_name {
                    pool,
                    hooks: <#hooks_ident as ::autumn_web::hooks::RepositoryHooksDefault>::autumn_default(),
                    idempotency: _parts
                        .extensions
                        .get::<::autumn_web::idempotency::IdempotencyContext>()
                        .cloned(),
                })
            }
        } else {
            quote! {
                Ok(#pg_name {
                    pool,
                    hooks: <#hooks_ident as ::autumn_web::hooks::RepositoryHooksDefault>::autumn_default(),
                })
            }
        };

        let hook_support_methods = if commit_hooks_enabled {
            quote! {
            #[doc(hidden)]
            fn __autumn_repository_commit_hook_key() -> &'static str {
                ::core::concat!(
                    ::core::env!("CARGO_PKG_NAME"),
                    "::",
                    ::core::module_path!(),
                    "::",
                    ::core::stringify!(#table_ident),
                    "::",
                    ::core::stringify!(#model_name),
                    "::",
                    ::core::stringify!(#hooks_ident)
                )
            }

            #[doc(hidden)]
            fn __autumn_register_repository_commit_hooks() {
                static __AUTUMN_REGISTERED: ::std::sync::OnceLock<()> = ::std::sync::OnceLock::new();
                __AUTUMN_REGISTERED.get_or_init(|| {
                    ::autumn_web::__private::register_repository_commit_hook_runner(
                        Self::__autumn_repository_commit_hook_key(),
                        |__ctx, __record| async move {
                            let mut __ctx: ::autumn_web::hooks::MutationContext =
                                ::autumn_web::reexports::serde_json::from_value(__ctx)
                                    .map_err(|__error| {
                                        ::autumn_web::AutumnError::internal_server_error_msg(
                                            format!("deserialize repository create hook context: {__error}")
                                        )
                                    })?;
                            let __record: #model_name =
                                #model_name::__autumn_commit_hook_from_value(__record)?;
                                let __hooks =
                                    <#hooks_ident as ::autumn_web::hooks::RepositoryHooksDefault>::autumn_default();
                                <#hooks_ident as ::autumn_web::hooks::MutationHooks>::after_create_commit(
                                    &__hooks,
                                    &mut __ctx,
                                    &__record,
                                )
                                .await
                        },
                        |__ctx, __record| async move {
                            let mut __ctx: ::autumn_web::hooks::MutationContext =
                                ::autumn_web::reexports::serde_json::from_value(__ctx)
                                    .map_err(|__error| {
                                        ::autumn_web::AutumnError::internal_server_error_msg(
                                            format!("deserialize repository update hook context: {__error}")
                                        )
                                    })?;
                            let __record: #model_name =
                                #model_name::__autumn_commit_hook_from_value(__record)?;
                                let __hooks =
                                    <#hooks_ident as ::autumn_web::hooks::RepositoryHooksDefault>::autumn_default();
                                <#hooks_ident as ::autumn_web::hooks::MutationHooks>::after_update_commit(
                                    &__hooks,
                                    &mut __ctx,
                                    &__record,
                                )
                                .await
                        },
                        |__ctx, __record| async move {
                            let mut __ctx: ::autumn_web::hooks::MutationContext =
                                ::autumn_web::reexports::serde_json::from_value(__ctx)
                                    .map_err(|__error| {
                                        ::autumn_web::AutumnError::internal_server_error_msg(
                                            format!("deserialize repository delete hook context: {__error}")
                                        )
                                    })?;
                            let __record: #model_name =
                                #model_name::__autumn_commit_hook_from_value(__record)?;
                                let __hooks =
                                    <#hooks_ident as ::autumn_web::hooks::RepositoryHooksDefault>::autumn_default();
                                <#hooks_ident as ::autumn_web::hooks::MutationHooks>::after_delete_commit(
                                    &__hooks,
                                    &mut __ctx,
                                    &__record,
                                )
                                .await
                        },
                    );
                });
            }
            }
        } else {
            quote! {}
        };

        let hook_inventory_registration = if commit_hooks_enabled {
            quote! {
                ::autumn_web::reexports::inventory::submit! {
                    ::autumn_web::__private::RepositoryCommitHookDescriptor {
                        register: #pg_name::__autumn_register_repository_commit_hooks,
                    }
                }
            }
        } else {
            quote! {}
        };

        // ── save (hooked) ─────────────────────────────────
        let save_body = if commit_hooks_enabled {
            quote! {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                use ::autumn_web::reexports::diesel_async::AsyncConnection;
                use ::autumn_web::reexports::scoped_futures::ScopedFutureExt as _;
                use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

            Self::__autumn_register_repository_commit_hooks();
            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            let (record, mut ctx, __autumn_commit_hook_id, __autumn_commit_hook_owner, __autumn_commit_hook_record) = conn
                .transaction::<(#model_name, MutationContext, ::std::string::String, ::std::string::String, ::autumn_web::reexports::serde_json::Value), ::autumn_web::AutumnError, _>(|conn| {
                    async move {
                        let mut input = new.clone();
                        let mut ctx = MutationContext::new(MutationOp::Create);
                        if let ::core::option::Option::Some(__autumn_idempotency) = &self.idempotency {
                            ctx.set_idempotency_key(__autumn_idempotency.scoped_key());
                        }

                        // before_create can validate/reject/rewrite
                        self.hooks.before_create(&mut ctx, &mut input).await?;

                        let record = ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
                            .values(&input)
                            .get_result::<#model_name>(conn)
                            .await
                            .map_err(::autumn_web::AutumnError::from)?;

                        let __autumn_commit_hook_record = record.__autumn_commit_hook_to_value()?;
                        let (__autumn_commit_hook_id, __autumn_commit_hook_owner) = ::autumn_web::__private::enqueue_repository_commit_hook_pending_on_conn(
                            conn,
                            Self::__autumn_repository_commit_hook_key(),
                            "create",
                            ctx.idempotency_key.as_deref(),
                            &ctx,
                            &__autumn_commit_hook_record,
                        )
                        .await?;

                        Ok((record, ctx, __autumn_commit_hook_id, __autumn_commit_hook_owner, __autumn_commit_hook_record))
                    }
                    .scope_boxed()
                })
                .await?;
            ::core::mem::drop(conn);
            let __autumn_pending_heartbeat =
                ::autumn_web::__private::start_repository_commit_hook_pending_finalizer_heartbeat(
                    self.pool.clone(),
                    __autumn_commit_hook_id.clone(),
                    __autumn_commit_hook_owner.clone(),
                );
            let __autumn_after_create = ::autumn_web::__private::catch_repository_after_hook_unwind(
                self.hooks.after_create(&mut ctx, &record)
            )
            .await;
            match __autumn_after_create {
                ::core::result::Result::Ok(::core::result::Result::Ok(())) => {}
                ::core::result::Result::Ok(::core::result::Result::Err(__autumn_error)) => {
                    let __autumn_error_message = ::std::format!("{__autumn_error}");
                    ::autumn_web::__private::mark_repository_commit_hook_after_hook_failed(
                        &self.pool,
                        &__autumn_commit_hook_id,
                        &__autumn_commit_hook_owner,
                        __autumn_error_message,
                    )
                    .await;
                    __autumn_pending_heartbeat.cancel();
                    return ::core::result::Result::Err(__autumn_error);
                }
                ::core::result::Result::Err(__autumn_panic) => {
                    ::autumn_web::__private::mark_repository_commit_hook_after_hook_failed(
                        &self.pool,
                        &__autumn_commit_hook_id,
                        &__autumn_commit_hook_owner,
                        "after_create panicked",
                    )
                    .await;
                    __autumn_pending_heartbeat.cancel();
                    ::std::panic::resume_unwind(__autumn_panic);
                }
            }
            let __autumn_finalize_result = ::autumn_web::__private::finalize_repository_commit_hook_after_hook(
                &self.pool,
                &__autumn_commit_hook_id,
                &__autumn_commit_hook_owner,
                &ctx,
                &__autumn_commit_hook_record,
            )
            .await;
            __autumn_pending_heartbeat.cancel();
            match __autumn_finalize_result {
                ::core::result::Result::Ok(()) => {
                    ::autumn_web::__private::kick_repository_commit_hook_dispatcher(&self.pool);
                }
                ::core::result::Result::Err(__autumn_error) => {
                    ::autumn_web::reexports::tracing::warn!(
                        hook_id = %__autumn_commit_hook_id,
                        error = %__autumn_error,
                        "failed to finalize repository create commit hook after mutation commit; leaving staged row for recovery"
                    );
                }
            }

                Ok(record)
            }
        } else {
            quote! {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                use ::autumn_web::reexports::diesel_async::AsyncConnection;
                use ::autumn_web::reexports::scoped_futures::ScopedFutureExt as _;
                use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                let (record, mut ctx) = conn
                    .transaction::<(#model_name, MutationContext), ::autumn_web::AutumnError, _>(|conn| {
                        async move {
                            let mut input = new.clone();
                            let mut ctx = MutationContext::new(MutationOp::Create);

                            self.hooks.before_create(&mut ctx, &mut input).await?;

                            let record = ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
                                .values(&input)
                                .get_result::<#model_name>(conn)
                                .await
                                .map_err(::autumn_web::AutumnError::from)?;

                            Ok((record, ctx))
                        }
                        .scope_boxed()
                    })
                    .await?;
                ::core::mem::drop(conn);
                self.hooks.after_create(&mut ctx, &record).await?;

                Ok(record)
            }
        };

        // ── update (hooked) ───────────────────────────────
        let draft_ext_trait = format_ident!("{}DraftExt", model_name);
        let update_body = if commit_hooks_enabled {
            quote! {
            use ::autumn_web::reexports::diesel::prelude::*;
            use ::autumn_web::reexports::diesel_async::RunQueryDsl;
            use ::autumn_web::reexports::diesel_async::AsyncConnection;
            use ::autumn_web::reexports::scoped_futures::ScopedFutureExt as _;
            use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks, UpdateDraft};
            use ::autumn_web::repository::{AutumnLockVersionModelExt as _, AutumnLockVersionUpdateExt as _};

            Self::__autumn_register_repository_commit_hooks();
            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            let (record, mut ctx, __autumn_commit_hook_id, __autumn_commit_hook_owner, __autumn_commit_hook_record) = conn
                .transaction::<(#model_name, MutationContext, ::std::string::String, ::std::string::String, ::autumn_web::reexports::serde_json::Value), ::autumn_web::AutumnError, _>(|conn| {
                    async move {
                        let mut ctx = MutationContext::new(MutationOp::Update);
                        if let ::core::option::Option::Some(__autumn_idempotency) = &self.idempotency {
                            ctx.set_idempotency_key(__autumn_idempotency.scoped_key());
                        }
                        let record: #model_name = if let ::core::option::Option::Some(expected_version) =
                            changes.__autumn_lock_version_expected()
                        {
                            // SELECT FOR UPDATE grabs an exclusive row lock so
                            // no concurrent writer can commit between our
                            // version check and the UPDATE below.
                            let current = #table_ident::table
                                .find(id)
                                .for_update()
                                .first::<#model_name>(conn)
                                .await
                                .optional()
                                .map_err(::autumn_web::AutumnError::from)?
                                .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                                    format!("{} with id {} not found", stringify!(#model_name), id)
                                ))?;

                            if let ::core::option::Option::Some(actual_version) =
                                current.__autumn_lock_version_actual()
                            {
                                if actual_version != expected_version {
                                    return Err(::autumn_web::AutumnError::conflict(
                                        ::autumn_web::RepositoryError::Conflict {
                                            id,
                                            expected_version,
                                            actual_version: ::core::option::Option::Some(actual_version),
                                        },
                                    ));
                                }
                            }

                            let mut draft = <UpdateDraft<#model_name> as #draft_ext_trait>::from_patch(&current, changes)?;
                            self.hooks.before_update(&mut ctx, &mut draft).await?;

                            let proposed = draft.into_after();
                            ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                                .set(&proposed)
                                .get_result::<#model_name>(conn)
                                .await
                                .map_err(::autumn_web::AutumnError::from)?
                        } else {
                            // Load current record
                            let current = #table_ident::table
                                .find(id)
                                .first::<#model_name>(conn)
                                .await
                                .optional()
                                .map_err(::autumn_web::AutumnError::from)?
                                .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                                    format!("{} with id {} not found", stringify!(#model_name), id)
                                ))?;

                            let mut draft = <UpdateDraft<#model_name> as #draft_ext_trait>::from_patch(&current, changes)?;
                            self.hooks.before_update(&mut ctx, &mut draft).await?;

                            let proposed = draft.into_after();
                            ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                                .set(&proposed)
                            .get_result::<#model_name>(conn)
                            .await
                            .map_err(::autumn_web::AutumnError::from)?
                        };

                        let __autumn_commit_hook_record = record.__autumn_commit_hook_to_value()?;
                        let (__autumn_commit_hook_id, __autumn_commit_hook_owner) = ::autumn_web::__private::enqueue_repository_commit_hook_pending_on_conn(
                            conn,
                            Self::__autumn_repository_commit_hook_key(),
                            "update",
                            ctx.idempotency_key.as_deref(),
                            &ctx,
                            &__autumn_commit_hook_record,
                        )
                        .await?;

                        Ok((record, ctx, __autumn_commit_hook_id, __autumn_commit_hook_owner, __autumn_commit_hook_record))
                    }
                    .scope_boxed()
                })
                .await?;
            ::core::mem::drop(conn);
            let __autumn_pending_heartbeat =
                ::autumn_web::__private::start_repository_commit_hook_pending_finalizer_heartbeat(
                    self.pool.clone(),
                    __autumn_commit_hook_id.clone(),
                    __autumn_commit_hook_owner.clone(),
                );
            let __autumn_after_update = ::autumn_web::__private::catch_repository_after_hook_unwind(
                self.hooks.after_update(&mut ctx, &record)
            )
            .await;
            match __autumn_after_update {
                ::core::result::Result::Ok(::core::result::Result::Ok(())) => {}
                ::core::result::Result::Ok(::core::result::Result::Err(__autumn_error)) => {
                    let __autumn_error_message = ::std::format!("{__autumn_error}");
                    ::autumn_web::__private::mark_repository_commit_hook_after_hook_failed(
                        &self.pool,
                        &__autumn_commit_hook_id,
                        &__autumn_commit_hook_owner,
                        __autumn_error_message,
                    )
                    .await;
                    __autumn_pending_heartbeat.cancel();
                    return ::core::result::Result::Err(__autumn_error);
                }
                ::core::result::Result::Err(__autumn_panic) => {
                    ::autumn_web::__private::mark_repository_commit_hook_after_hook_failed(
                        &self.pool,
                        &__autumn_commit_hook_id,
                        &__autumn_commit_hook_owner,
                        "after_update panicked",
                    )
                    .await;
                    __autumn_pending_heartbeat.cancel();
                    ::std::panic::resume_unwind(__autumn_panic);
                }
            }
            let __autumn_finalize_result = ::autumn_web::__private::finalize_repository_commit_hook_after_hook(
                &self.pool,
                &__autumn_commit_hook_id,
                &__autumn_commit_hook_owner,
                &ctx,
                &__autumn_commit_hook_record,
            )
            .await;
            __autumn_pending_heartbeat.cancel();
            match __autumn_finalize_result {
                ::core::result::Result::Ok(()) => {
                    ::autumn_web::__private::kick_repository_commit_hook_dispatcher(&self.pool);
                }
                ::core::result::Result::Err(__autumn_error) => {
                    ::autumn_web::reexports::tracing::warn!(
                        hook_id = %__autumn_commit_hook_id,
                        error = %__autumn_error,
                        "failed to finalize repository update commit hook after mutation commit; leaving staged row for recovery"
                    );
                }
            }

                Ok(record)
            }
        } else {
            quote! {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                use ::autumn_web::reexports::diesel_async::AsyncConnection;
                use ::autumn_web::reexports::scoped_futures::ScopedFutureExt as _;
                use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks, UpdateDraft};
                use ::autumn_web::repository::{AutumnLockVersionModelExt as _, AutumnLockVersionUpdateExt as _};

                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                let (record, mut ctx) = conn
                    .transaction::<(#model_name, MutationContext), ::autumn_web::AutumnError, _>(|conn| {
                        async move {
                            let mut ctx = MutationContext::new(MutationOp::Update);
                            let record: #model_name = if let ::core::option::Option::Some(expected_version) =
                                changes.__autumn_lock_version_expected()
                            {
                                let current = #table_ident::table
                                    .find(id)
                                    .for_update()
                                    .first::<#model_name>(conn)
                                    .await
                                    .optional()
                                    .map_err(::autumn_web::AutumnError::from)?
                                    .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                                        format!("{} with id {} not found", stringify!(#model_name), id)
                                    ))?;

                                if let ::core::option::Option::Some(actual_version) =
                                    current.__autumn_lock_version_actual()
                                {
                                    if actual_version != expected_version {
                                        return Err(::autumn_web::AutumnError::conflict(
                                            ::autumn_web::RepositoryError::Conflict {
                                                id,
                                                expected_version,
                                                actual_version: ::core::option::Option::Some(actual_version),
                                            },
                                        ));
                                    }
                                }

                                let mut draft = <UpdateDraft<#model_name> as #draft_ext_trait>::from_patch(&current, changes)?;
                                self.hooks.before_update(&mut ctx, &mut draft).await?;

                                let proposed = draft.into_after();
                                ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                                    .set(&proposed)
                                    .get_result::<#model_name>(conn)
                                    .await
                                    .map_err(::autumn_web::AutumnError::from)?
                            } else {
                                let current = #table_ident::table
                                    .find(id)
                                    .first::<#model_name>(conn)
                                    .await
                                    .optional()
                                    .map_err(::autumn_web::AutumnError::from)?
                                    .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                                        format!("{} with id {} not found", stringify!(#model_name), id)
                                    ))?;

                                let mut draft = <UpdateDraft<#model_name> as #draft_ext_trait>::from_patch(&current, changes)?;
                                self.hooks.before_update(&mut ctx, &mut draft).await?;

                                let proposed = draft.into_after();
                                ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                                    .set(&proposed)
                                    .get_result::<#model_name>(conn)
                                    .await
                                    .map_err(::autumn_web::AutumnError::from)?
                            };

                            Ok((record, ctx))
                        }
                        .scope_boxed()
                    })
                    .await?;
                ::core::mem::drop(conn);
                self.hooks.after_update(&mut ctx, &record).await?;

                Ok(record)
            }
        };

        // ── delete (hooked) ───────────────────────────────
        let delete_body = if commit_hooks_enabled {
            quote! {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                use ::autumn_web::reexports::diesel_async::AsyncConnection;
                use ::autumn_web::reexports::scoped_futures::ScopedFutureExt as _;
                use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

            Self::__autumn_register_repository_commit_hooks();
            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
            conn
                .transaction::<(), ::autumn_web::AutumnError, _>(|conn| {
                    async move {
                        let mut ctx = MutationContext::new(MutationOp::Delete);
                        if let ::core::option::Option::Some(__autumn_idempotency) = &self.idempotency {
                            ctx.set_idempotency_key(__autumn_idempotency.scoped_key());
                        }

                        // Load current record for before_delete context
                        let record = #table_ident::table
                            .find(id)
                            .for_update()
                            .first::<#model_name>(conn)
                            .await
                            .optional()
                            .map_err(::autumn_web::AutumnError::from)?
                            .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                                format!("{} with id {} not found", stringify!(#model_name), id)
                            ))?;

                        self.hooks.before_delete(&mut ctx, &record).await?;

                        let __autumn_deleted = ::autumn_web::reexports::diesel::delete(#table_ident::table.find(id))
                            .execute(conn)
                            .await
                            .map_err(::autumn_web::AutumnError::from)?;
                        if __autumn_deleted == 0 {
                            return Err(::autumn_web::AutumnError::not_found_msg(
                                format!("{} with id {} not found", stringify!(#model_name), id)
                            ));
                        }

                        let __autumn_commit_hook_record = record.__autumn_commit_hook_to_value()?;
                        ::autumn_web::__private::enqueue_repository_commit_hook_on_conn(
                            conn,
                            Self::__autumn_repository_commit_hook_key(),
                            "delete",
                            ctx.idempotency_key.as_deref(),
                            &ctx,
                            &__autumn_commit_hook_record,
                        )
                        .await?;

                        Ok(())
                    }
                    .scope_boxed()
                })
                .await?;
            ::core::mem::drop(conn);
            ::autumn_web::__private::kick_repository_commit_hook_dispatcher(&self.pool);

                Ok(())
            }
        } else {
            quote! {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                use ::autumn_web::reexports::diesel_async::AsyncConnection;
                use ::autumn_web::reexports::scoped_futures::ScopedFutureExt as _;
                use ::autumn_web::hooks::{MutationContext, MutationOp, MutationHooks};

                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn
                    .transaction::<(), ::autumn_web::AutumnError, _>(|conn| {
                        async move {
                            let mut ctx = MutationContext::new(MutationOp::Delete);

                            let record = #table_ident::table
                                .find(id)
                                .for_update()
                                .first::<#model_name>(conn)
                                .await
                                .optional()
                                .map_err(::autumn_web::AutumnError::from)?
                                .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                                    format!("{} with id {} not found", stringify!(#model_name), id)
                                ))?;

                            self.hooks.before_delete(&mut ctx, &record).await?;

                            let __autumn_deleted = ::autumn_web::reexports::diesel::delete(#table_ident::table.find(id))
                                .execute(conn)
                                .await
                                .map_err(::autumn_web::AutumnError::from)?;
                            if __autumn_deleted == 0 {
                                return Err(::autumn_web::AutumnError::not_found_msg(
                                    format!("{} with id {} not found", stringify!(#model_name), id)
                                ));
                            }

                            Ok(())
                        }
                        .scope_boxed()
                    })
                    .await?;
                ::core::mem::drop(conn);

                Ok(())
            }
        };

        (
            struct_fields,
            clone_impl,
            extractor_init,
            save_body,
            update_body,
            delete_body,
            hook_support_methods,
            hook_inventory_registration,
        )
    } else {
        // ── No hooks: existing zero-cost path ─────────────

        let struct_fields = quote! {
            pool: ::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<
                ::autumn_web::reexports::diesel_async::AsyncPgConnection,
            >,
        };

        let clone_impl = quote! {
            impl ::core::clone::Clone for #pg_name {
                fn clone(&self) -> Self {
                    Self {
                        pool: self.pool.clone(),
                    }
                }
            }
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
            use ::autumn_web::repository::{AutumnLockVersionModelExt as _, AutumnLockVersionUpdateExt as _};
            let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;

            if let ::core::option::Option::Some(expected_version) =
                changes.__autumn_lock_version_expected()
            {
                use ::autumn_web::reexports::diesel_async::AsyncConnection;
                use ::autumn_web::reexports::scoped_futures::ScopedFutureExt as _;

                conn.transaction::<_, ::autumn_web::AutumnError, _>(|conn| {
                    async move {
                        // SELECT FOR UPDATE grabs an exclusive row lock so
                        // no concurrent writer can commit between our
                        // version check and the UPDATE below.
                        let current = #table_ident::table
                            .find(id)
                            .for_update()
                            .first::<#model_name>(conn)
                            .await
                            .optional()
                            .map_err(::autumn_web::AutumnError::from)?
                            .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                                format!("{} with id {} not found", stringify!(#model_name), id)
                            ))?;

                        if let ::core::option::Option::Some(actual_version) =
                            current.__autumn_lock_version_actual()
                        {
                            if actual_version != expected_version {
                                return Err(::autumn_web::AutumnError::conflict(
                                    ::autumn_web::RepositoryError::Conflict {
                                        id,
                                        expected_version,
                                        actual_version: ::core::option::Option::Some(actual_version),
                                    },
                                ));
                            }
                        }

                        let diesel_changeset = changes.__to_changeset();
                        ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                            .set(&diesel_changeset)
                            .get_result::<#model_name>(conn)
                            .await
                            .map_err(::autumn_web::AutumnError::from)
                    }
                    .scope_boxed()
                })
                .await
            } else {
                let diesel_changeset = changes.__to_changeset();
                ::autumn_web::reexports::diesel::update(#table_ident::table.find(id))
                    .set(&diesel_changeset)
                    .get_result::<#model_name>(&mut conn)
                    .await
                    .map_err(::autumn_web::AutumnError::from)
            }
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
            clone_impl,
            extractor_init,
            save_body,
            update_body,
            delete_body,
            quote! {},
            quote! {},
        )
    };

    let route_hook_registration = if commit_hooks_enabled {
        quote! { #pg_name::__autumn_register_repository_commit_hooks(); }
    } else {
        quote! {}
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

        let list_path_fn = format_ident!("__autumn_path_{prefix}_api_list");
        let get_path_fn = format_ident!("__autumn_path_{prefix}_api_get");
        let create_path_fn = format_ident!("__autumn_path_{prefix}_api_create");
        let update_path_fn = format_ident!("__autumn_path_{prefix}_api_update");
        let delete_path_fn = format_ident!("__autumn_path_{prefix}_api_delete");

        let id_path = format!("{api_path}/{{id}}");

        let has_policy = config.policy_type.is_some();
        let policy_check_show = if has_policy {
            quote! {
                ::autumn_web::authorization::__check_policy::<#model_name>(
                    &__autumn_state,
                    &__autumn_session,
                    "show",
                    &record,
                )
                .await?;
            }
        } else {
            quote! {}
        };
        // POST endpoint runs `can_create` *before* the insert so a
        // denied check never commits a row. Naive after-the-fact
        // policy checks would write the row, then return 403/404,
        // leaving the data behind.
        let policy_check_create_pre = if has_policy {
            quote! {
                ::autumn_web::authorization::__check_policy_create_payload::<#model_name>(
                    &__autumn_state,
                    &__autumn_session,
                    &__autumn_new_payload,
                )
                .await?;
            }
        } else {
            quote! {}
        };
        // Policy-backed create handlers keep the raw JSON value for
        // `can_create_payload` instead of serializing `NewModel` back
        // into JSON. That preserves hand-written `NewModel` types that
        // are `Deserialize + Insertable` but intentionally not `Serialize`.
        let create_payload_arg = if has_policy {
            quote! {
                ::autumn_web::prelude::Json(__autumn_new_payload): ::autumn_web::prelude::Json<
                    ::autumn_web::reexports::serde_json::Value
                >
            }
        } else {
            quote! {
                ::autumn_web::prelude::Json(new): ::autumn_web::prelude::Json<#new_name>
            }
        };
        let decode_create_payload = if has_policy {
            quote! {
                let new: #new_name = ::autumn_web::reexports::serde_json::from_value(
                    __autumn_new_payload.clone(),
                )
                .map_err(|err| ::autumn_web::AutumnError::unprocessable_msg(err.to_string()))?;
            }
        } else {
            quote! {}
        };
        let policy_check_update_pre = if has_policy {
            quote! {
                let __existing = repo.find_by_id(id).await?
                    .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg("not found"))?;
                ::autumn_web::authorization::__check_policy::<#model_name>(
                    &__autumn_state,
                    &__autumn_session,
                    "update",
                    &__existing,
                )
                .await?;
            }
        } else {
            quote! {}
        };
        let policy_check_delete_pre = if has_policy {
            quote! {
                let __existing = repo.find_by_id(id).await?
                    .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg("not found"))?;
                ::autumn_web::authorization::__check_policy::<#model_name>(
                    &__autumn_state,
                    &__autumn_session,
                    "delete",
                    &__existing,
                )
                .await?;
            }
        } else {
            quote! {}
        };
        let session_state_args = if has_policy {
            quote! {
                ::autumn_web::reexports::axum::extract::State(__autumn_state):
                    ::autumn_web::reexports::axum::extract::State<::autumn_web::AppState>,
                __autumn_session: ::autumn_web::session::Session,
            }
        } else {
            quote! {}
        };
        // List endpoint behavior, in order of precedence:
        //
        // 1. `scope = SomeScope`: invoke the registered scope (the
        //    most efficient form — the scope filters at the SQL level
        //    via Diesel).
        // 2. `policy = SomePolicy` without `scope`: load every
        //    record, then filter through `Policy::can_show` per row.
        //    Slower than (1) for large tables, but closes the
        //    "policy guards show/update/delete but list returns
        //    everything" data-exposure path. Users who care about
        //    perf should also set `scope = SomeScope`.
        // 3. Neither: plain `repo.find_all()` (public list).
        let scope_list_body = if config.scope_type.is_some() {
            quote! {
                let __scope = __autumn_state
                    .scope::<#model_name>()
                    .ok_or_else(|| ::autumn_web::AutumnError::internal_server_error_msg(
                        "missing scope registration"
                    ))?;
                let __ctx = ::autumn_web::authorization::PolicyContext::from_request(
                    &__autumn_state,
                    &__autumn_session,
                ).await;
                let mut __conn = repo.__autumn_acquire_conn().await?;
                let records = __scope.list(&__ctx, &mut __conn).await?;
                Ok(::autumn_web::prelude::Json(records))
            }
        } else if has_policy {
            quote! {
                let __policy = __autumn_state
                    .policy::<#model_name>()
                    .ok_or_else(|| ::autumn_web::AutumnError::internal_server_error_msg(
                        "missing policy registration"
                    ))?;
                let __ctx = ::autumn_web::authorization::PolicyContext::from_request(
                    &__autumn_state,
                    &__autumn_session,
                ).await;
                let __all = repo.find_all().await?;
                let mut __filtered = ::std::vec::Vec::with_capacity(__all.len());
                for __record in __all {
                    if __policy.can_show(&__ctx, &__record).await {
                        __filtered.push(__record);
                    }
                }
                Ok(::autumn_web::prelude::Json(__filtered))
            }
        } else {
            quote! {
                Ok(::autumn_web::prelude::Json(repo.find_all().await?))
            }
        };
        // Inject session + state extractors when *either* a scope
        // or a policy is configured — both code paths above need
        // them.
        let list_session_state_args = if config.scope_type.is_some() || has_policy {
            quote! {
                ::autumn_web::reexports::axum::extract::State(__autumn_state):
                    ::autumn_web::reexports::axum::extract::State<::autumn_web::AppState>,
                __autumn_session: ::autumn_web::session::Session,
            }
        } else {
            quote! {}
        };
        let resource_type_name_lit = model_name.to_string();
        let api_path_lit = api_path.clone();

        // Compile-time assertion: when the user writes
        // `policy = SomePolicy`, the generated code references the
        // type so a typo (or a real type that doesn't `impl
        // Policy<Model>`) fails compilation here, not at the first
        // request with `500 missing policy registration`.
        let policy_type_assertion = if let Some(ref policy_type) = config.policy_type {
            quote! {
                const _: fn() = || {
                    fn __autumn_assert_policy<P: ::autumn_web::authorization::Policy<#model_name>>() {}
                    __autumn_assert_policy::<#policy_type>();
                };
            }
        } else {
            quote! {}
        };
        // Emit a type-erased registry probe so the app builder can
        // verify at startup that the policy was actually registered
        // via `.policy::<R, _>(...)`. Without this, forgetting the
        // `.policy(...)` call would compile and boot, then 500 on
        // every protected request.
        let policy_check_fn = if config.policy_type.is_some() {
            quote! {
                ::core::option::Option::Some(
                    (|registry: &::autumn_web::authorization::PolicyRegistry| {
                        registry.has_policy::<#model_name>()
                    }) as fn(&::autumn_web::authorization::PolicyRegistry) -> bool
                )
            }
        } else {
            quote! { ::core::option::Option::None }
        };
        // Companion probe for `scope = ...`. ONLY attached to the
        // `_api_list` route's metadata — the other auto-generated
        // routes (`*_api_get` / `*_api_create` / `*_api_update` /
        // `*_api_delete`) never call `scope.list`, so flagging them
        // for missing scope registration would fire the prod fail-
        // fast even when the user intentionally mounted only
        // non-list endpoints with `scope = ...` configured (the
        // app's reads happen via custom queries, but the scope is
        // still declared so `Note::scope(&ctx)` works in hand-
        // written list handlers). The non-list routes below get
        // `scope_check: None` regardless.
        let list_scope_check_fn = if config.scope_type.is_some() {
            quote! {
                ::core::option::Option::Some(
                    (|registry: &::autumn_web::authorization::PolicyRegistry| {
                        registry.scope::<#model_name>().is_some()
                    }) as fn(&::autumn_web::authorization::PolicyRegistry) -> bool
                )
            }
        } else {
            quote! { ::core::option::Option::None }
        };
        let non_list_scope_check_fn = quote! { ::core::option::Option::None };
        let scope_type_assertion = if let Some(ref scope_type) = config.scope_type {
            quote! {
                const _: fn() = || {
                    fn __autumn_assert_scope<S: ::autumn_web::authorization::Scope<#model_name>>() {}
                    __autumn_assert_scope::<#scope_type>();
                };
            }
        } else {
            quote! {}
        };

        quote! {
            // ── Auto-generated REST API handlers ─────────────────

            #policy_type_assertion
            #scope_type_assertion

            #vis async fn #list_fn(
                #list_session_state_args
                repo: #pg_name,
            ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<Vec<#model_name>>> {
                #scope_list_body
            }

            #[doc(hidden)]
            #vis fn #list_info() -> ::autumn_web::Route {
                #route_hook_registration
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::GET,
                    path: #api_path,
                    handler: ::autumn_web::reexports::axum::routing::MethodRouter::<
                        ::autumn_web::AppState, ::core::convert::Infallible
                    >::layer(
                        ::autumn_web::reexports::axum::routing::get(#list_fn),
                        ::autumn_web::idempotency::IdempotencyReplayLayer,
                    ),
                    name: ::core::stringify!(#list_fn),
                    api_doc: ::autumn_web::openapi::ApiDoc {
                        method: "GET",
                        path: #api_path,
                        operation_id: ::core::stringify!(#list_fn),
                        success_status: 200,
                        response: ::core::option::Option::Some(
                            ::autumn_web::openapi::SchemaEntry {
                                name: "array",
                                kind: ::autumn_web::openapi::SchemaKind::Array(
                                    &::autumn_web::openapi::SchemaEntry {
                                        name: ::core::stringify!(#model_name),
                                        kind: ::autumn_web::openapi::SchemaKind::Ref,
                                    }
                                ),
                            }
                        ),
                        ..::core::default::Default::default()
                    },
                    repository: ::core::option::Option::Some(::autumn_web::RepositoryApiMeta {
                        resource_type_name: #resource_type_name_lit,
                        api_path: #api_path_lit,
                        has_policy: #has_policy,
                        policy_check: #policy_check_fn,
                        scope_check: #list_scope_check_fn,
                    }),
                }
            }

            #vis async fn #get_fn(
                #session_state_args
                ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
                repo: #pg_name,
            ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<#model_name>> {
                let record = repo.find_by_id(id).await?
                    .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg("not found"))?;
                #policy_check_show
                Ok(::autumn_web::prelude::Json(record))
            }

            #[doc(hidden)]
            #vis fn #get_info() -> ::autumn_web::Route {
                #route_hook_registration
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::GET,
                    path: #id_path,
                    handler: ::autumn_web::reexports::axum::routing::MethodRouter::<
                        ::autumn_web::AppState, ::core::convert::Infallible
                    >::layer(
                        ::autumn_web::reexports::axum::routing::get(#get_fn),
                        ::autumn_web::idempotency::IdempotencyReplayLayer,
                    ),
                    name: ::core::stringify!(#get_fn),
                    api_doc: ::autumn_web::openapi::ApiDoc {
                        method: "GET",
                        path: #id_path,
                        operation_id: ::core::stringify!(#get_fn),
                        path_params: &["id"],
                        success_status: 200,
                        response: ::core::option::Option::Some(
                            ::autumn_web::openapi::SchemaEntry {
                                name: ::core::stringify!(#model_name),
                                kind: ::autumn_web::openapi::SchemaKind::Ref,
                            }
                        ),
                        ..::core::default::Default::default()
                    },
                    repository: ::core::option::Option::Some(::autumn_web::RepositoryApiMeta {
                        resource_type_name: #resource_type_name_lit,
                        api_path: #api_path_lit,
                        has_policy: #has_policy,
                        policy_check: #policy_check_fn,
                        scope_check: #non_list_scope_check_fn,
                    }),
                }
            }

            #vis async fn #create_fn(
                #session_state_args
                repo: #pg_name,
                #create_payload_arg,
            ) -> ::autumn_web::AutumnResult<(::autumn_web::reexports::http::StatusCode, ::autumn_web::prelude::Json<#model_name>)> {
                #decode_create_payload
                #policy_check_create_pre
                let record = repo.save(&new).await?;
                Ok((::autumn_web::reexports::http::StatusCode::CREATED, ::autumn_web::prelude::Json(record)))
            }

            #[doc(hidden)]
            #vis fn #create_info() -> ::autumn_web::Route {
                #route_hook_registration
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::POST,
                    path: #api_path,
                    handler: ::autumn_web::reexports::axum::routing::MethodRouter::<
                        ::autumn_web::AppState, ::core::convert::Infallible
                    >::layer(
                        ::autumn_web::reexports::axum::routing::post(#create_fn),
                        ::autumn_web::idempotency::IdempotencyReplayLayer,
                    ),
                    name: ::core::stringify!(#create_fn),
                    api_doc: ::autumn_web::openapi::ApiDoc {
                        method: "POST",
                        path: #api_path,
                        operation_id: ::core::stringify!(#create_fn),
                        success_status: 201,
                        request_body: ::core::option::Option::Some(
                            ::autumn_web::openapi::SchemaEntry {
                                name: ::core::stringify!(#new_name),
                                kind: ::autumn_web::openapi::SchemaKind::Ref,
                            }
                        ),
                        response: ::core::option::Option::Some(
                            ::autumn_web::openapi::SchemaEntry {
                                name: ::core::stringify!(#model_name),
                                kind: ::autumn_web::openapi::SchemaKind::Ref,
                            }
                        ),
                        ..::core::default::Default::default()
                    },
                    repository: ::core::option::Option::Some(::autumn_web::RepositoryApiMeta {
                        resource_type_name: #resource_type_name_lit,
                        api_path: #api_path_lit,
                        has_policy: #has_policy,
                        policy_check: #policy_check_fn,
                        scope_check: #non_list_scope_check_fn,
                    }),
                }
            }

            #vis async fn #update_fn(
                #session_state_args
                ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
                repo: #pg_name,
                ::autumn_web::prelude::Json(patch): ::autumn_web::prelude::Json<#update_name>,
            ) -> ::autumn_web::AutumnResult<::autumn_web::prelude::Json<#model_name>> {
                #policy_check_update_pre
                let record = repo.update(id, &patch).await?;
                Ok(::autumn_web::prelude::Json(record))
            }

            #[doc(hidden)]
            #vis fn #update_info() -> ::autumn_web::Route {
                #route_hook_registration
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::PUT,
                    path: #id_path,
                    handler: ::autumn_web::reexports::axum::routing::MethodRouter::<
                        ::autumn_web::AppState, ::core::convert::Infallible
                    >::layer(
                        ::autumn_web::reexports::axum::routing::put(#update_fn),
                        ::autumn_web::idempotency::IdempotencyReplayLayer,
                    ),
                    name: ::core::stringify!(#update_fn),
                    api_doc: ::autumn_web::openapi::ApiDoc {
                        method: "PUT",
                        path: #id_path,
                        operation_id: ::core::stringify!(#update_fn),
                        path_params: &["id"],
                        success_status: 200,
                        request_body: ::core::option::Option::Some(
                            ::autumn_web::openapi::SchemaEntry {
                                name: ::core::stringify!(#update_name),
                                kind: ::autumn_web::openapi::SchemaKind::Ref,
                            }
                        ),
                        response: ::core::option::Option::Some(
                            ::autumn_web::openapi::SchemaEntry {
                                name: ::core::stringify!(#model_name),
                                kind: ::autumn_web::openapi::SchemaKind::Ref,
                            }
                        ),
                        ..::core::default::Default::default()
                    },
                    repository: ::core::option::Option::Some(::autumn_web::RepositoryApiMeta {
                        resource_type_name: #resource_type_name_lit,
                        api_path: #api_path_lit,
                        has_policy: #has_policy,
                        policy_check: #policy_check_fn,
                        scope_check: #non_list_scope_check_fn,
                    }),
                }
            }

            #vis async fn #delete_fn(
                #session_state_args
                ::autumn_web::extract::Path(id): ::autumn_web::extract::Path<i64>,
                repo: #pg_name,
            ) -> ::autumn_web::AutumnResult<::autumn_web::reexports::http::StatusCode> {
                #policy_check_delete_pre
                repo.delete_by_id(id).await?;
                Ok(::autumn_web::reexports::http::StatusCode::NO_CONTENT)
            }

            #[doc(hidden)]
            #vis fn #delete_info() -> ::autumn_web::Route {
                #route_hook_registration
                ::autumn_web::Route {
                    method: ::autumn_web::reexports::http::Method::DELETE,
                    path: #id_path,
                    handler: ::autumn_web::reexports::axum::routing::MethodRouter::<
                        ::autumn_web::AppState, ::core::convert::Infallible
                    >::layer(
                        ::autumn_web::reexports::axum::routing::delete(#delete_fn),
                        ::autumn_web::idempotency::IdempotencyReplayLayer,
                    ),
                    name: ::core::stringify!(#delete_fn),
                    api_doc: ::autumn_web::openapi::ApiDoc {
                        method: "DELETE",
                        path: #id_path,
                        operation_id: ::core::stringify!(#delete_fn),
                        path_params: &["id"],
                        success_status: 204,
                        ..::core::default::Default::default()
                    },
                    repository: ::core::option::Option::Some(::autumn_web::RepositoryApiMeta {
                        resource_type_name: #resource_type_name_lit,
                        api_path: #api_path_lit,
                        has_policy: #has_policy,
                        policy_check: #policy_check_fn,
                        scope_check: #non_list_scope_check_fn,
                    }),
                }
            }

            // ── Path helpers for API routes ───────────────────────

            #[doc(hidden)]
            #vis fn #list_path_fn() -> ::std::string::String {
                #api_path.to_owned()
            }

            #[doc(hidden)]
            #vis fn #get_path_fn(id: impl ::std::fmt::Display) -> ::std::string::String {
                format!("{}/{}", #api_path, ::autumn_web::paths::encode_path_segment(id))
            }

            #[doc(hidden)]
            #vis fn #create_path_fn() -> ::std::string::String {
                #api_path.to_owned()
            }

            #[doc(hidden)]
            #vis fn #update_path_fn(id: impl ::std::fmt::Display) -> ::std::string::String {
                format!("{}/{}", #api_path, ::autumn_web::paths::encode_path_segment(id))
            }

            #[doc(hidden)]
            #vis fn #delete_path_fn(id: impl ::std::fmt::Display) -> ::std::string::String {
                format!("{}/{}", #api_path, ::autumn_web::paths::encode_path_segment(id))
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
        #vis struct #pg_name {
            #struct_fields
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

        #clone_impl

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

        impl #pg_name {
            #hook_support_methods

            /// Acquire a database connection from the repository's
            /// pool. Used by `#[repository(scope = ...)]`-generated
            /// list endpoints; not part of the public surface.
            #[doc(hidden)]
            pub async fn __autumn_acquire_conn(
                &self,
            ) -> ::autumn_web::AutumnResult<
                ::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Object<
                    ::autumn_web::reexports::diesel_async::AsyncPgConnection,
                >,
            > {
                self.pool.get().await.map_err(::autumn_web::AutumnError::from)
            }

            /// Pessimistic lock helper: SELECT FOR UPDATE the row with
            /// the given `id` inside a transaction, then call `f` with
            /// the locked record and the transaction connection.
            ///
            /// Returns `404 Not Found` if no row with `id` exists.
            pub async fn with_lock<F, T>(&self, id: i64, f: F) -> ::autumn_web::AutumnResult<T>
            where
                F: for<'c> ::core::ops::FnOnce(
                    #model_name,
                    &'c mut ::autumn_web::reexports::diesel_async::AsyncPgConnection,
                ) -> ::autumn_web::reexports::scoped_futures::ScopedBoxFuture<'c, 'c, ::autumn_web::AutumnResult<T>>
                    + ::core::marker::Send + 'static,
                T: ::core::marker::Send + 'static,
            {
                use ::autumn_web::reexports::diesel::prelude::*;
                use ::autumn_web::reexports::diesel_async::RunQueryDsl;
                use ::autumn_web::reexports::diesel_async::AsyncConnection;
                use ::autumn_web::reexports::scoped_futures::ScopedFutureExt as _;

                let mut conn = self.pool.get().await.map_err(::autumn_web::AutumnError::from)?;
                conn.transaction::<T, ::autumn_web::AutumnError, _>(|conn| {
                    async move {
                        let row = #table_ident::table
                            .find(id)
                            .for_update()
                            .first::<#model_name>(conn)
                            .await
                            .optional()
                            .map_err(::autumn_web::AutumnError::from)?
                            .ok_or_else(|| ::autumn_web::AutumnError::not_found_msg(
                                format!("{} with id {} not found", stringify!(#model_name), id)
                            ))?;
                        f(row, conn).await
                    }
                    .scope_boxed()
                })
                .await
            }
        }

        #hook_inventory_registration

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
        assert!(
            !config.commit_hooks,
            "ordinary hooks must not opt into the durable commit-hook queue"
        );
    }

    #[test]
    fn parse_repo_args_with_commit_hooks() {
        let tokens: proc_macro2::TokenStream = "Post, hooks = PostHooks, commit_hooks = true"
            .parse()
            .unwrap();
        let config = parse_repo_args(tokens).unwrap();
        assert_eq!(config.model_name.to_string(), "Post");
        assert!(config.hooks_type.is_some());
        assert!(config.commit_hooks);
    }

    #[test]
    fn parse_repo_args_rejects_commit_hooks_without_hooks() {
        let tokens: proc_macro2::TokenStream = "Post, commit_hooks = true".parse().unwrap();
        let Err(err) = parse_repo_args(tokens) else {
            panic!("commit hooks require a hook type");
        };
        assert!(
            err.to_string().contains("requires hooks"),
            "unexpected error: {err}"
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
    fn hooked_repository_without_commit_hooks_uses_ordinary_hooks_only() {
        let output = repository_macro(
            quote! { Post, hooks = PostHooks },
            quote! { pub trait PostRepository {} },
        );
        let generated = output.to_string();

        assert!(
            generated.contains("self . hooks . before_create")
                && generated.contains("self . hooks . after_create")
                && generated.contains("self . hooks . before_update")
                && generated.contains("self . hooks . after_update"),
            "ordinary hooks should still be generated: {generated}"
        );
        assert!(
            !generated.contains("enqueue_repository_commit_hook")
                && !generated.contains("RepositoryCommitHookDescriptor")
                && !generated.contains("__autumn_register_repository_commit_hooks"),
            "ordinary hooks must not require or dispatch through the durable commit-hook queue: {generated}"
        );
    }

    fn durable_hook_repository_tokens() -> String {
        repository_macro(
            quote! { Post, hooks = PostHooks, commit_hooks = true },
            quote! { pub trait PostRepository {} },
        )
        .to_string()
    }

    #[test]
    fn hooked_repository_commit_hooks_register_durable_runner_when_opted_in() {
        let generated = durable_hook_repository_tokens();

        assert!(
            generated.contains("IdempotencyContext"),
            "generated commit-hook repositories must extract the request idempotency context: {generated}"
        );
        assert!(
            generated.contains("ctx . set_idempotency_key"),
            "generated commit-hook repositories must seed MutationContext with the scoped idempotency key: {generated}"
        );
        assert!(
            generated.contains("ctx . idempotency_key . as_deref ()"),
            "generated commit-hook rows must use the scoped idempotency key for durable dedupe: {generated}"
        );
        assert!(
            generated.contains("enqueue_repository_commit_hook_pending_on_conn"),
            "generated repositories must durably stage after_*_commit hooks before the mutation commits: {generated}"
        );
        assert!(
            generated.contains("finalize_repository_commit_hook_after_hook"),
            "generated repositories must only make staged after_*_commit hooks dispatchable after regular after hooks succeed: {generated}"
        );
        assert!(
            generated.contains("mark_repository_commit_hook_after_hook_failed"),
            "generated repositories must mark staged after_*_commit hooks non-dispatchable when regular after hooks fail: {generated}"
        );
        assert!(
            generated.contains("RepositoryCommitHookDescriptor"),
            "generated repositories must register hook runners at link time: {generated}"
        );
        assert!(
            !generated.contains("register_after_commit"),
            "generated after_*_commit hooks must not use the process-local callback registry"
        );
        assert!(
            generated.contains("module_path ! ()"),
            "durable hook handler keys must include the repository module path to avoid cross-module runner collisions: {generated}"
        );
        assert!(
            generated.contains("env ! (\"CARGO_PKG_NAME\")"),
            "durable hook handler keys should include a stable package/table/model identity: {generated}"
        );
        assert!(
            generated.contains("__autumn_commit_hook_to_value")
                && generated.contains("__autumn_commit_hook_from_value"),
            "generated repository hook runners must use the framework's full-fidelity record codec: {generated}"
        );
        assert!(
            !generated.contains("serde_json :: from_value (__record)"),
            "generated repository hook runners must not rehydrate records through public serde JSON: {generated}"
        );
        assert!(
            !generated.contains("self . hooks . after_create (& mut ctx , & record) . await ?"),
            "after_create errors must be reported without rolling back the inserted record: {generated}"
        );
        assert!(
            !generated.contains("self . hooks . after_update (& mut ctx , & record) . await ?"),
            "after_update errors must be reported without rolling back the updated record: {generated}"
        );
    }

    #[test]
    fn hooked_repository_commit_hook_finalization_failures_are_non_fatal() {
        let generated = durable_hook_repository_tokens();

        assert!(
            !generated.contains("__autumn_finalize_result ?"),
            "post-commit finalization failures must not make save/update report the committed mutation as failed: {generated}"
        );
        assert!(
            generated
                .contains("failed to finalize repository create commit hook after mutation commit")
                && generated.contains(
                    "failed to finalize repository update commit hook after mutation commit"
                ),
            "post-commit finalization failures should be logged and left for stale-pending recovery: {generated}"
        );
    }

    #[test]
    fn hooked_repository_create_commit_hooks_finalize_after_regular_after_hook() {
        let generated = durable_hook_repository_tokens();

        let create_stage = generated
            .find(
                "\"create\" , ctx . idempotency_key . as_deref () , & ctx , & __autumn_commit_hook_record",
            )
            .expect("create commit hook staging should use the encoded record");
        let create_after = generated
            .find("self . hooks . after_create (& mut ctx , & record)")
            .expect("after_create hook should still be generated");
        let create_drop_conn = generated
            .find(":: core :: mem :: drop (conn)")
            .expect("create path should release the repository connection before after/finalize");
        let create_finalize = generated
            .find("finalize_repository_commit_hook_after_hook (& self . pool , & __autumn_commit_hook_id")
            .expect("create commit hook should be finalized after after_create succeeds");
        assert!(
            create_stage < create_after,
            "create commit hook rows must be staged inside the mutation transaction: {generated}"
        );
        assert!(
            create_stage < create_drop_conn && create_drop_conn < create_after,
            "create path must release its checked-out connection before after_create/finalize checks out from the pool: {generated}"
        );
        assert!(
            create_after < create_finalize,
            "after_create_commit dispatch must see the finalized MutationContext from after_create: {generated}"
        );
        let create_failure_mark = generated
            .find("mark_repository_commit_hook_after_hook_failed")
            .expect("after_create failure path should mark the staged row as non-dispatchable");
        let create_cancel = generated
            .find("__autumn_pending_heartbeat . cancel ()")
            .expect("after_create path should cancel the pending heartbeat");
        assert!(
            generated.contains("catch_repository_after_hook_unwind")
                && generated.contains(":: std :: panic :: resume_unwind"),
            "after_create panics must be caught long enough to discard staged commit hooks before unwinding: {generated}"
        );
        assert!(
            create_failure_mark < create_cancel,
            "after_create failure must mark the staged row before canceling its heartbeat: {generated}"
        );
    }

    #[test]
    fn hooked_repository_update_commit_hooks_finalize_after_regular_after_hook() {
        let generated = durable_hook_repository_tokens();

        let update_stage = generated
            .find(
                "\"update\" , ctx . idempotency_key . as_deref () , & ctx , & __autumn_commit_hook_record",
            )
            .expect("update commit hook staging should use the encoded record");
        let update_after = generated
            .find("self . hooks . after_update (& mut ctx , & record)")
            .expect("after_update hook should still be generated");
        let update_drop_conn = generated[update_stage..update_after]
            .find(":: core :: mem :: drop (conn)")
            .map(|idx| update_stage + idx)
            .expect("update path should release the repository connection before after/finalize");
        let update_finalize = generated
            .rfind("finalize_repository_commit_hook_after_hook (& self . pool , & __autumn_commit_hook_id")
            .expect("update commit hook should be finalized after after_update succeeds");
        assert!(
            update_stage < update_after,
            "update commit hook rows must be staged inside the mutation transaction: {generated}"
        );
        assert!(
            update_stage < update_drop_conn && update_drop_conn < update_after,
            "update path must release its checked-out connection before after_update/finalize checks out from the pool: {generated}"
        );
        assert!(
            update_after < update_finalize,
            "after_update_commit dispatch must see the finalized MutationContext from after_update: {generated}"
        );
        let update_failure_mark = generated[update_after..]
            .find("mark_repository_commit_hook_after_hook_failed")
            .map(|idx| update_after + idx)
            .expect("after_update failure path should mark the staged row as non-dispatchable");
        let update_cancel = generated[update_after..]
            .find("__autumn_pending_heartbeat . cancel ()")
            .map(|idx| update_after + idx)
            .expect("after_update path should cancel the pending heartbeat");
        assert!(
            update_failure_mark < update_cancel,
            "after_update failure must mark the staged row before canceling its heartbeat: {generated}"
        );
    }

    #[test]
    fn hooked_repository_delete_commit_hooks_lock_and_check_deleted_count() {
        let generated = durable_hook_repository_tokens();

        let delete_start = generated
            .find("MutationOp :: Delete")
            .expect("delete path should still be generated");
        let delete_generated = &generated[delete_start..];
        let delete_lock = delete_generated
            .find(". for_update ()")
            .expect("delete path should lock the row before before_delete");
        let before_delete = delete_generated
            .find("before_delete")
            .expect("before_delete hook should still be generated");
        assert!(
            delete_lock < before_delete,
            "delete path must lock the row before invoking before_delete: {generated}"
        );
        assert!(
            generated.contains("let __autumn_deleted =")
                && generated.contains("if __autumn_deleted == 0"),
            "delete path must not enqueue after_delete_commit when no row was deleted: {generated}"
        );
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
