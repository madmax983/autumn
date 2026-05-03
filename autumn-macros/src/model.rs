//! `#[model]` attribute macro implementation.
//!
//! Generates four types from a single struct:
//! - The query type (original struct) with `Queryable`, `Selectable`
//! - A `NewX` insert type with `Insertable` (ID fields excluded)
//! - An `UpdateX` patch type with `Default` (ID fields excluded, all `Patch<T>`)
//! - A `XField` enum with one variant per mutable field (for audit/CDC payloads)
//!
//! Also generates on `UpdateDraft<Model>`:
//! - `from_patch(current, patch)` — merges a `Patch`-based update into a draft
//! - Per-field `DraftField` accessor methods for inspecting/overriding changes
//!
//! Recognises `#[id]`, `#[indexed]`, and `#[validate(...)]` field attributes.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser as _;
use syn::{DeriveInput, Field, LitStr};

/// Process `#[model]` or `#[model(table = "...")]` attribute arguments.
fn parse_attr_args(attr: TokenStream) -> syn::Result<Option<String>> {
    if attr.is_empty() {
        return Ok(None);
    }

    let mut table = None;
    syn::meta::parser(|meta| {
        if meta.path.is_ident("table") {
            let value: LitStr = meta.value()?.parse()?;
            table = Some(value.value());
            Ok(())
        } else {
            Err(meta.error("unsupported model attribute"))
        }
    })
    .parse2(attr)?;

    Ok(table)
}

/// Check if a field has the `#[id]` attribute.
fn has_attr(field: &Field, name: &str) -> bool {
    field.attrs.iter().any(|a| a.path().is_ident(name))
}

/// Extract `#[validate(...)]` attributes from a field (verbatim pass-through).
fn validate_attrs(field: &Field) -> Vec<&syn::Attribute> {
    field
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("validate"))
        .collect()
}

/// Filter out framework-specific attributes (`#[id]`, `#[indexed]`, `#[validate]`,
/// `#[default]`, `#[factory_assoc]`) that shouldn't be on the query struct (they'd confuse Diesel derives).
fn user_attrs(field: &Field) -> Vec<&syn::Attribute> {
    field
        .attrs
        .iter()
        .filter(|a| {
            !a.path().is_ident("id")
                && !a.path().is_ident("indexed")
                && !a.path().is_ident("validate")
                && !a.path().is_ident("default")
                && !a.path().is_ident("factory_assoc")
        })
        .collect()
}

/// Extract the associated model type from `#[factory_assoc(TypeName)]` if present.
///
/// Returns `Some(Ident)` for the associated type, or `None` if the attribute is absent.
fn factory_assoc_type(field: &Field) -> Option<syn::Ident> {
    for attr in &field.attrs {
        if attr.path().is_ident("factory_assoc") {
            if let Ok(ident) = attr.parse_args::<syn::Ident>() {
                return Some(ident);
            }
        }
    }
    None
}

/// True if a field has `#[id]` or `#[default]` — either way it's excluded
/// from the `NewX` insert type.
fn excluded_from_new(field: &Field) -> bool {
    has_attr(field, "id") || has_attr(field, "default")
}

/// Convert a `snake_case` identifier to `PascalCase`.
fn pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_uppercase().to_string() + &chars.collect::<String>()
            })
        })
        .collect()
}

/// Check whether a type is `Option<...>`.
fn is_option_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(tp) = ty {
        tp.path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Option")
    } else {
        false
    }
}

#[allow(clippy::too_many_lines)]
pub fn model_macro(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse2(item) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    let syn::Data::Struct(syn::DataStruct {
        fields: syn::Fields::Named(ref fields),
        ..
    }) = input.data
    else {
        return syn::Error::new_spanned(
            &input.ident,
            "#[model] can only be applied to structs with named fields",
        )
        .to_compile_error();
    };

    let table_name = match parse_attr_args(attr) {
        Ok(explicit) => explicit.unwrap_or_else(|| infer_table_name(&input.ident)),
        Err(err) => return err.to_compile_error(),
    };

    let table_ident = syn::Ident::new(&table_name, input.ident.span());
    let name = &input.ident;
    let vis = &input.vis;
    let outer_attrs = &input.attrs;

    let new_name = format_ident!("New{name}");
    let update_name = format_ident!("Update{name}");

    // Classify fields
    let all_fields: Vec<&Field> = fields.named.iter().collect();
    let id_fields: Vec<&&Field> = all_fields.iter().filter(|f| has_attr(f, "id")).collect();

    // If no explicit #[id], default to first i32/i64 field
    let id_field_names: Vec<&syn::Ident> = if id_fields.is_empty() {
        all_fields
            .iter()
            .filter(|f| {
                if let syn::Type::Path(tp) = &f.ty {
                    tp.path.is_ident("i32") || tp.path.is_ident("i64")
                } else {
                    false
                }
            })
            .take(1)
            .filter_map(|f| f.ident.as_ref())
            .collect()
    } else {
        id_fields.iter().filter_map(|f| f.ident.as_ref()).collect()
    };

    // Fields for NewX: exclude #[id], #[default], and auto-detected ID fields
    let fields_for_new: Vec<&&Field> = all_fields
        .iter()
        .filter(|f| {
            !excluded_from_new(f)
                && f.ident
                    .as_ref()
                    .is_some_and(|id| !id_field_names.contains(&id))
        })
        .collect();

    // Fields for UpdateX: same set as NewX (all become Patch<T>)

    // Check if any field has #[validate(...)]
    let has_validation = all_fields.iter().any(|f| !validate_attrs(f).is_empty());

    // Build query struct fields (strip #[id], #[indexed], #[validate])
    let query_fields: Vec<TokenStream> = all_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            let attrs = user_attrs(f);
            quote! { #(#attrs)* pub #ident: #ty }
        })
        .collect();

    // Build NewX fields (non-ID, propagate #[validate])
    let new_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            let val_attrs = validate_attrs(f);
            quote! { #(#val_attrs)* pub #ident: #ty }
        })
        .collect();

    // Build UpdateX fields (non-ID, all Patch<T>; no #[validate] — validation
    // doesn't apply to Patch<T> fields, only to NewX and the merged model)
    let update_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            quote! {
                #[serde(default)]
                pub #ident: ::autumn_web::hooks::Patch<#ty>
            }
        })
        .collect();

    // Build XField enum variants (one per mutable field, PascalCase)
    let field_enum_name = format_ident!("{name}Field");
    let field_enum_variants: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let variant = format_ident!("{}", pascal_case(&ident.to_string()));
            quote! { #variant }
        })
        .collect();

    // Conditional Validate derive
    let validate_derive = if has_validation {
        quote! { #[derive(::autumn_web::reexports::validator::Validate)] }
    } else {
        quote! {}
    };

    // Build merge arms for `from_patch` (applies Patch fields onto a cloned model)
    let merge_arms: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let is_option = is_option_type(&f.ty);
            if is_option {
                quote! {
                    match &patch.#ident {
                        ::autumn_web::hooks::Patch::Set(v) => after.#ident = v.clone(),
                        ::autumn_web::hooks::Patch::Clear => after.#ident = None,
                        ::autumn_web::hooks::Patch::Unchanged => {}
                    }
                }
            } else {
                quote! {
                    match &patch.#ident {
                        ::autumn_web::hooks::Patch::Set(v) => after.#ident = v.clone(),
                        ::autumn_web::hooks::Patch::Clear => {
                            return Err(::autumn_web::AutumnError::bad_request_msg(
                                format!("Cannot clear non-nullable field `{}`", stringify!(#ident))
                            ));
                        }
                        ::autumn_web::hooks::Patch::Unchanged => {}
                    }
                }
            }
        })
        .collect();

    // Build per-field DraftField accessor method signatures (for the trait)
    let draft_accessor_sigs: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let ty = &f.ty;
            quote! {
                fn #ident(&mut self) -> ::autumn_web::hooks::DraftField<'_, #ty>;
            }
        })
        .collect();

    // Build per-field DraftField accessor method implementations
    let draft_accessors: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let ty = &f.ty;
            quote! {
                fn #ident(&mut self) -> ::autumn_web::hooks::DraftField<'_, #ty> {
                    ::autumn_web::hooks::DraftField::new(&self.before.#ident, &mut self.after.#ident)
                }
            }
        })
        .collect();

    // Trait name for draft extension methods
    let draft_ext_name = format_ident!("{name}DraftExt");

    // Build Diesel-compatible changeset bridge (private struct with Option<T> fields)
    let changeset_name = format_ident!("__{}Changeset", name);

    let changeset_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            // For both nullable and non-nullable columns, Diesel's AsChangeset
            // treats Option<T> as "skip if None, set if Some". For nullable
            // columns (Option<Inner>), this becomes Option<Option<Inner>> which
            // also handles "set to NULL" via Some(None).
            quote! { pub #ident: Option<#ty> }
        })
        .collect();

    let changeset_conversions: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let is_option = is_option_type(&f.ty);
            if is_option {
                // For nullable fields: Set(v) -> Some(v), Clear -> Some(None), Unchanged -> None
                quote! {
                    #ident: match &self.#ident {
                        ::autumn_web::hooks::Patch::Set(v) => Some(v.clone()),
                        ::autumn_web::hooks::Patch::Clear => Some(None),
                        ::autumn_web::hooks::Patch::Unchanged => None,
                    }
                }
            } else {
                // For non-nullable fields: Set(v) -> Some(v), Unchanged -> None, Clear -> panic
                quote! {
                    #ident: match &self.#ident {
                        ::autumn_web::hooks::Patch::Set(v) => Some(v.clone()),
                        ::autumn_web::hooks::Patch::Clear => {
                            panic!("Cannot clear non-nullable field `{}`", stringify!(#ident));
                        }
                        ::autumn_web::hooks::Patch::Unchanged => None,
                    }
                }
            }
        })
        .collect();

    // ── Factory builder ────────────────────────────────────────
    let factory_name = format_ident!("{name}Factory");

    // Whether any factory field is an association (drives depth-check generation).
    let has_assoc_fields = fields_for_new
        .iter()
        .any(|f| factory_assoc_type(f).is_some());

    // Factory struct fields.
    // - Normal fields:  `pub {ident}: {ty}`
    // - Assoc fields:   `pub {ident}: Option<{ty}>` (None = auto-create on create())
    let factory_struct_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            if factory_assoc_type(f).is_some() {
                quote! { pub #ident: ::core::option::Option<#ty> }
            } else {
                quote! { pub #ident: #ty }
            }
        })
        .collect();

    // Default impl.
    // - Normal fields:  `{ident}: Default::default()`
    // - Assoc fields:   `{ident}: None`
    let factory_default_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            if factory_assoc_type(f).is_some() {
                quote! { #ident: ::core::option::Option::None }
            } else {
                quote! { #ident: ::core::default::Default::default() }
            }
        })
        .collect();

    // Per-field setter methods.
    // - Normal fields:  `pub fn {ident}(mut self, val: impl Into<T>) -> Self`
    // - Assoc fields:   same setter (stores `Some(val.into())`), PLUS
    //                   `pub fn {assoc_snake}(mut self, val: &AssocType) -> Self`
    //                   that extracts `.id` from a pre-built instance.
    let factory_setters: Vec<TokenStream> = fields_for_new
        .iter()
        .flat_map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let ty = &f.ty;

            if let Some(assoc_type) = factory_assoc_type(f) {
                // Setter that stores an explicit id via Some(...)
                let explicit_setter = quote! {
                    #[must_use]
                    pub fn #ident(mut self, val: impl ::core::convert::Into<#ty>) -> Self {
                        self.#ident = ::core::option::Option::Some(val.into());
                        self
                    }
                };
                // Setter that accepts a pre-built associated instance and extracts `.id`.
                // Name is derived from the field ident by stripping the `_id` suffix:
                // `user_id` → `.user()`, `author_id` → `.author()`.
                let field_str = ident.to_string();
                let assoc_snake = if field_str.ends_with("_id") {
                    format_ident!("{}", &field_str[..field_str.len() - 3])
                } else {
                    format_ident!("{}_assoc", field_str)
                };
                let pre_built_setter = quote! {
                    /// Override the association with a pre-built instance.
                    /// Extracts `.id` so no additional DB insert is performed on `create()`.
                    #[must_use]
                    pub fn #assoc_snake(mut self, val: &#assoc_type) -> Self {
                        self.#ident = ::core::option::Option::Some(val.id);
                        self
                    }
                };
                vec![explicit_setter, pre_built_setter]
            } else {
                vec![quote! {
                    #[must_use]
                    pub fn #ident(mut self, val: impl ::core::convert::Into<#ty>) -> Self {
                        self.#ident = val.into();
                        self
                    }
                }]
            }
        })
        .collect();

    // build() — assemble NewX.
    // - Normal fields:  `{ident}: self.{ident}`
    // - Assoc fields:   `{ident}: self.{ident}.unwrap_or_default()`
    let factory_build_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = &f.ident;
            if factory_assoc_type(f).is_some() {
                quote! { #ident: self.#ident.unwrap_or_default() }
            } else {
                quote! { #ident: self.#ident }
            }
        })
        .collect();

    // create() — auto-resolve assoc fields, then insert.
    //
    // For each assoc field, emit a `let __resolved_{ident}` that either uses the
    // supplied value or auto-creates the associated model via its factory.
    //
    // A thread-local depth counter guards against cyclic associations: if the
    // chain exceeds 32 levels the factory panics with a clear message rather than
    // overflowing the stack.
    let assoc_resolutions: Vec<TokenStream> = fields_for_new
        .iter()
        .filter_map(|f| {
            let assoc_type = factory_assoc_type(f)?;
            let ident = f.ident.as_ref().unwrap();
            let resolved = format_ident!("__resolved_{ident}");
            Some(quote! {
                let #resolved = match self.#ident {
                    ::core::option::Option::Some(id) => id,
                    ::core::option::Option::None => {
                        #assoc_type::factory().create(pool).await.id
                    }
                };
            })
        })
        .collect();

    // NewX construction inside create() uses resolved values for assoc fields.
    let create_build_fields: Vec<TokenStream> = fields_for_new
        .iter()
        .map(|f| {
            let ident = f.ident.as_ref().unwrap();
            if factory_assoc_type(f).is_some() {
                let resolved = format_ident!("__resolved_{ident}");
                quote! { #ident: #resolved }
            } else {
                quote! { #ident: self.#ident }
            }
        })
        .collect();

    // Depth-guard block — only emitted when there are assoc fields.
    let depth_guard_enter = if has_assoc_fields {
        quote! {
            ::std::thread_local! {
                static __AUTUMN_FACTORY_DEPTH: ::std::cell::Cell<u32> =
                    ::std::cell::Cell::new(0);
            }
            __AUTUMN_FACTORY_DEPTH.with(|d| {
                let v = d.get();
                assert!(
                    v < 32,
                    "factory `{}`: cyclic #[factory_assoc] chain exceeds depth 32 — \
                     break the cycle by supplying a pre-built instance via a \
                     pre-built setter.",
                    stringify!(#name),
                );
                d.set(v + 1);
            });
        }
    } else {
        quote! {}
    };
    let depth_guard_exit = if has_assoc_fields {
        quote! { __AUTUMN_FACTORY_DEPTH.with(|d| d.set(d.get() - 1)); }
    } else {
        quote! {}
    };

    // create() — insert via Diesel and return the persisted model.
    let factory_create_method = quote! {
        /// Insert a record built from this factory into the database and return
        /// the fully-populated model (with server-assigned primary key).
        ///
        /// Fields annotated with `#[factory_assoc(Type)]` are auto-created via
        /// `Type::factory().create(pool).await` when no explicit value was set.
        /// Supply a pre-built instance with the `.{type_snake}(instance)` setter
        /// to skip the extra insert.
        ///
        /// Panics if the insert fails or if a cyclic association chain is detected
        /// (depth > 32). Intended for use in tests only.
        pub async fn create(
            self,
            pool: &::autumn_web::reexports::diesel_async::pooled_connection::deadpool::Pool<
                ::autumn_web::reexports::diesel_async::AsyncPgConnection,
            >,
        ) -> #name {
            #depth_guard_enter
            use ::autumn_web::reexports::diesel::prelude::*;
            use ::autumn_web::reexports::diesel_async::RunQueryDsl;

            #(#assoc_resolutions)*

            let new_record = #new_name {
                #(#create_build_fields,)*
            };
            let mut conn = pool
                .get()
                .await
                .expect("factory: failed to acquire db connection");
            let result = ::autumn_web::reexports::diesel::insert_into(#table_ident::table)
                .values(&new_record)
                .returning(#name::as_returning())
                .get_result(&mut *conn)
                .await
                .expect("factory: insert failed");
            #depth_guard_exit
            result
        }
    };

    quote! {
        #[derive(Debug, Clone, ::diesel::Queryable, ::diesel::Selectable, ::diesel::AsChangeset)]
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        #[diesel(table_name = #table_ident)]
        #(#outer_attrs)*
        #vis struct #name {
            #(#query_fields,)*
        }

        #[derive(Debug, Clone, ::diesel::Insertable)]
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        #validate_derive
        #[diesel(table_name = #table_ident)]
        #vis struct #new_name {
            #(#new_fields,)*
        }

        #[derive(Debug, Clone, Default)]
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        #vis struct #update_name {
            #(#update_fields,)*
        }

        /// Diesel-compatible changeset derived from `Patch<T>` fields.
        ///
        /// This type bridges the `Patch`-based `UpdateX` and Diesel's
        /// `AsChangeset` trait. Use `UpdateX::__to_changeset()` to convert.
        #[doc(hidden)]
        #[derive(Debug, Clone, ::diesel::AsChangeset)]
        #[diesel(table_name = #table_ident)]
        pub struct #changeset_name {
            #(#changeset_fields,)*
        }

        impl #update_name {
            #[doc(hidden)]
            #[must_use]
            pub fn __to_changeset(&self) -> #changeset_name {
                #changeset_name {
                    #(#changeset_conversions,)*
                }
            }
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #vis enum #field_enum_name {
            #(#field_enum_variants,)*
        }

        /// Extension trait providing `from_patch` and per-field `DraftField` accessors
        /// for `UpdateDraft<#name>`.
        ///
        /// Generated by `#[model]`. Import this trait to call `from_patch()` or
        /// field accessor methods on `UpdateDraft<#name>`.
        #vis trait #draft_ext_name {
            /// Build a draft by merging the current record with a patch.
            ///
            /// Returns `Err` if a non-nullable field has `Patch::Clear`.
            fn from_patch(current: &#name, patch: &#update_name) -> ::autumn_web::AutumnResult<::autumn_web::hooks::UpdateDraft<#name>>;

            #(#draft_accessor_sigs)*
        }

        impl #draft_ext_name for ::autumn_web::hooks::UpdateDraft<#name> {
            fn from_patch(current: &#name, patch: &#update_name) -> ::autumn_web::AutumnResult<Self> {
                let mut after = current.clone();
                #(#merge_arms)*
                Ok(Self::new_with_changes(current.clone(), after))
            }

            #(#draft_accessors)*
        }

        /// Test-data factory builder for [`#name`].
        ///
        /// Produced by [`#name::factory()`]. All fields are pre-filled with
        /// `Default::default()` so tests only need to specify the fields that
        /// matter for the scenario under test.
        ///
        /// # Examples
        ///
        /// ```rust,ignore
        /// // Zero required args — all fields use sensible type defaults.
        /// let record = MyModel::factory().build();
        ///
        /// // Override only the fields relevant to your test.
        /// let record = MyModel::factory()
        ///     .title("Hello")
        ///     .published(true)
        ///     .build();
        ///
        /// // Persist to the database (returns the fully-populated model with PK).
        /// let persisted = MyModel::factory().title("Hello").create(&pool).await;
        /// ```
        #[derive(Debug, Clone)]
        #vis struct #factory_name {
            #(#factory_struct_fields,)*
        }

        impl ::core::default::Default for #factory_name {
            fn default() -> Self {
                Self {
                    #(#factory_default_fields,)*
                }
            }
        }

        impl #factory_name {
            #(#factory_setters)*

            /// Build a [`#new_name`] instance from the current factory state.
            ///
            /// Does not touch the database. Use [`#factory_name::create`] to
            /// also persist the record.
            #[must_use]
            pub fn build(self) -> #new_name {
                #new_name {
                    #(#factory_build_fields,)*
                }
            }

            #factory_create_method
        }

        impl #name {
            /// Create a factory builder for constructing [`#name`] instances in tests.
            ///
            /// All fields start at their [`Default`] value. Override any subset
            /// with the fluent setter methods, then call
            /// [`build()`](#factory_name::build) for an in-memory instance or
            /// [`create(pool)`](#factory_name::create) to persist it.
            ///
            /// # Example
            ///
            /// ```rust,ignore
            /// let record = MyModel::factory().title("Hello").build();
            /// ```
            #[must_use]
            pub fn factory() -> #factory_name {
                #factory_name::default()
            }
        }
    }
}

pub fn infer_table_name(ident: &syn::Ident) -> String {
    let name = ident.to_string();
    let snake = pascal_to_snake(&name);
    format!("{snake}s")
}

pub fn pascal_to_snake(s: &str) -> String {
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_ascii_lowercase());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_to_snake_simple() {
        assert_eq!(pascal_to_snake("User"), "user");
    }

    #[test]
    fn pascal_to_snake_multi_word() {
        assert_eq!(pascal_to_snake("BlogPost"), "blog_post");
    }

    #[test]
    fn pascal_to_snake_three_words() {
        assert_eq!(
            pascal_to_snake("UserProfileSettings"),
            "user_profile_settings"
        );
    }

    #[test]
    fn pascal_case_simple() {
        assert_eq!(pascal_case("title"), "Title");
    }

    #[test]
    fn pascal_case_multi_word() {
        assert_eq!(pascal_case("approved_at"), "ApprovedAt");
    }

    #[test]
    fn pascal_case_single_char() {
        assert_eq!(pascal_case("x"), "X");
    }

    #[test]
    fn infer_table_name_simple() {
        let ident = syn::Ident::new("User", proc_macro2::Span::call_site());
        assert_eq!(infer_table_name(&ident), "users");
    }

    #[test]
    fn infer_table_name_multi_word() {
        let ident = syn::Ident::new("BlogPost", proc_macro2::Span::call_site());
        assert_eq!(infer_table_name(&ident), "blog_posts");
    }
}
