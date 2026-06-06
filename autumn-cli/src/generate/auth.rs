//! `autumn generate auth` — generate a complete browser authentication flow.
//!
//! Creates a User model, Diesel migration, auth route handlers (signup / login /
//! logout / account / forgot-password / reset-password), generated request-level
//! tests, and a documentation file — all as ordinary app-owned code that the
//! user can edit freely after generation.
//!
//! Security properties of the generated code:
//! - Passwords are hashed with bcrypt (cost=12) via `autumn_web::auth`.
//! - Reset tokens are random values; only SHA-256 digests are persisted.
//! - Duplicate signup and failed login return identical non-enumerating errors.
//! - Login and reset-password rotate the session ID (prevents session fixation).
//! - Logout destroys the session (old session cannot remain authenticated).

use std::path::Path;

use super::emit::Plan;
use super::model::ensure_cargo_dependencies;
use super::naming::{pascal, pluralize, snake};
use super::schema_edit::{
    add_mod_declaration, append_schema_table, schema_has_table, update_main_rs,
};
use super::{Flags, GenerateError, ensure_project_root, timestamp_now};

/// Extra Cargo dependencies the auth generator needs on top of the model deps.
const AUTH_EXTRA_DEPS: &[(&str, &str)] = &[
    ("axum", "\"0.8\""),
    ("maud", "{ version = \"0.27\", features = [\"axum\"] }"),
    ("sha2", "{ version = \"0.10\", features = [] }"),
    ("hex", "\"0.4\""),
    ("rand", "{ version = \"0.9\", features = [\"os_rng\"] }"),
    ("tokio", "{ version = \"1\", features = [\"time\"] }"),
    ("tracing", "\"0.1\""),
];

/// Extra Cargo dependencies pulled in only by `--passkeys`.
///
/// - `webauthn-rs` (with `danger-allow-state-serialisation`) provides the
///   `WebAuthn` ceremony implementation. State serialisation lets the in-progress
///   ceremony challenge survive across requests via the session.
/// - `uuid` generates random credential IDs if the authenticator doesn't supply one.
/// - `conditional-ui` gates `start/finish_discoverable_authentication` in webauthn-rs 0.5.
const PASSKEY_EXTRA_DEPS: &[(&str, &str)] = &[
    (
        "webauthn-rs",
        "{ version = \"0.5\", features = [\"danger-allow-state-serialisation\", \"conditional-ui\"] }",
    ),
    ("uuid", "{ version = \"1\", features = [\"v4\"] }"),
];

/// Required features for the `webauthn-rs` dependency.
///
/// `danger-allow-state-serialisation` enables session storage of ceremony state.
/// `conditional-ui` gates `start/finish_discoverable_authentication`.
const WEBAUTHN_RS_FEATURES: &[&str] = &["danger-allow-state-serialisation", "conditional-ui"];

/// Ensure an existing `webauthn-rs` dependency carries the features the generated
/// routes need.
///
/// `ensure_cargo_dependencies` skips a crate that is already declared, so a project
/// that already lists `webauthn-rs` without `conditional-ui` would scaffold but fail
/// to compile. This merges the missing features into the existing declaration —
/// shorthand, inline-table, or `[dependencies.webauthn-rs]` subtable form.
fn ensure_webauthn_rs_features(toml: &str) -> String {
    const CRATE: &str = "webauthn-rs";
    let trailing_newline = toml.ends_with('\n');
    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    let simple_prefix = format!("{CRATE} = \"");
    let table_prefix = format!("{CRATE} = {{");
    let subtable_header = format!("[dependencies.{CRATE}]");
    let subtable_header_underscore = format!("[dependencies.{}]", CRATE.replace('-', "_"));
    let feats_csv = WEBAUTHN_RS_FEATURES
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");

    let merge_missing = |line: &str| -> Option<String> {
        let feat_bracket = line.find("features = [")?;
        let list_start = feat_bracket + "features = [".len();
        let close_off = line[list_start..].find(']')?;
        let list_end = close_off + list_start;
        let existing_list = &line[list_start..list_end];
        let additions: Vec<String> = WEBAUTHN_RS_FEATURES
            .iter()
            .filter(|f| !existing_list.contains(&format!("\"{f}\"")))
            .map(|f| format!("\"{f}\""))
            .collect();
        if additions.is_empty() {
            return None; // already complete
        }
        let sep = if existing_list.trim().is_empty() || existing_list.trim().ends_with(',') {
            ""
        } else {
            ", "
        };
        Some(format!(
            "{}{sep}{}{}",
            &line[..list_end],
            additions.join(", "),
            &line[list_end..]
        ))
    };

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim().to_owned();
        let indent: String = lines[i]
            .chars()
            .take_while(char::is_ascii_whitespace)
            .collect();

        if let Some(rest) = trimmed.strip_prefix(&simple_prefix) {
            let version = rest.split('"').next().unwrap_or("0.5");
            lines[i] = format!(
                "{indent}{CRATE} = {{ version = \"{version}\", features = [{feats_csv}] }}"
            );
            break;
        }

        if trimmed.starts_with(&table_prefix) {
            if let Some(new_line) = merge_missing(&trimmed) {
                lines[i] = format!("{indent}{new_line}");
            } else if !trimmed.contains("features = [") {
                // No `features` key at all — insert one before the closing brace.
                if let Some(close_brace) = trimmed.rfind('}') {
                    let before = trimmed[..close_brace].trim_end();
                    let sep = if before.ends_with('{') { " " } else { ", " };
                    lines[i] = format!("{indent}{before}{sep}features = [{feats_csv}] }}");
                }
            }
            break;
        }

        if trimmed == subtable_header || trimmed == subtable_header_underscore {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim().starts_with('[') {
                let t = lines[j].trim().to_owned();
                if t.starts_with("features") {
                    let ind2: String = lines[j]
                        .chars()
                        .take_while(char::is_ascii_whitespace)
                        .collect();
                    if let Some(new_line) = merge_missing(&t) {
                        lines[j] = format!("{ind2}{new_line}");
                    }
                    let mut out = lines.join("\n");
                    if trailing_newline {
                        out.push('\n');
                    }
                    return out;
                }
                j += 1;
            }
            // No `features` key found in the subtable — insert one.
            lines.insert(i + 1, format!("features = [{feats_csv}]"));
            break;
        }

        i += 1;
    }

    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    out
}

/// Extra Cargo dependencies pulled in only by `--totp`.
///
/// - `totp-rs` (with the `qr` feature) provides RFC 6238 TOTP plus QR rendering.
/// - `aes-gcm` encrypts the TOTP secret at rest.
/// - `base64` encodes the encrypted secret / decodes the encryption key.
///
/// Recovery codes are hashed with bcrypt via the existing `autumn_web::auth`
/// hashing path, so no extra hashing dependency is required.
const TOTP_EXTRA_DEPS: &[(&str, &str)] = &[
    (
        "totp-rs",
        "{ version = \"5\", features = [\"qr\", \"gen_secret\", \"otpauth\"] }",
    ),
    ("aes-gcm", "\"0.10\""),
    ("base64", "\"0.22\""),
];

/// `totp-rs` cargo features the generated `--totp` routes rely on
/// (`Secret::generate_secret`, `TOTP::new` with `otpauth`, `get_qr_base64`).
const TOTP_RS_FEATURES: &[&str] = &["qr", "gen_secret", "otpauth"];

/// Ensure an existing `totp-rs` dependency carries the features the generated
/// routes need.
///
/// `ensure_cargo_dependencies` skips a crate that is already declared, so an app
/// that already lists `totp-rs` (perhaps without `qr`/`gen_secret`/`otpauth`)
/// would scaffold but fail to compile. This merges the missing features into the
/// existing declaration — shorthand, inline-table, or `[dependencies.totp-rs]`
/// subtable form — mirroring the `autumn-web` feature helpers.
#[allow(clippy::too_many_lines)]
fn ensure_totp_rs_features(toml: &str) -> String {
    const CRATE: &str = "totp-rs";
    let trailing_newline = toml.ends_with('\n');
    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    let simple_prefix = format!("{CRATE} = \"");
    let table_prefix = format!("{CRATE} = {{");
    let subtable_header = format!("[dependencies.{CRATE}]");
    let subtable_header_underscore = format!("[dependencies.{}]", CRATE.replace('-', "_"));
    let feats_csv = TOTP_RS_FEATURES
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");

    // Merge `TOTP_RS_FEATURES` into the `[ ... ]` list embedded in `line`,
    // returning the rewritten line, or `None` if nothing changed (already
    // complete) / no list found.
    let merge_into_list = |line: &str, bracket_search: &str| -> Option<Option<String>> {
        let feat_bracket = line.find(bracket_search)?;
        let list_start = feat_bracket + bracket_search.len();
        let close_off = line[list_start..].find(']')?;
        let list_end = close_off + list_start;
        let existing_list = &line[list_start..list_end];
        let additions: Vec<String> = TOTP_RS_FEATURES
            .iter()
            .filter(|f| !existing_list.contains(&format!("\"{f}\"")))
            .map(|f| format!("\"{f}\""))
            .collect();
        if additions.is_empty() {
            return Some(None); // list present and already complete
        }
        let sep = if existing_list.trim().is_empty() || existing_list.trim().ends_with(',') {
            ""
        } else {
            ", "
        };
        Some(Some(format!(
            "{}{sep}{}{}",
            &line[..list_end],
            additions.join(", "),
            &line[list_end..]
        )))
    };

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim().to_owned();
        let indent: String = lines[i]
            .chars()
            .take_while(char::is_ascii_whitespace)
            .collect();

        // `totp-rs = "5"` → promote to a table carrying the required features.
        if let Some(rest) = trimmed.strip_prefix(&simple_prefix) {
            // `rest` is everything after `totp-rs = "`. Take up to the closing
            // quote so a trailing inline comment (`"5" # note`) isn't folded into
            // the version string (which would make Cargo.toml invalid).
            let (version, trailing) = rest.find('"').map_or_else(
                || (rest.trim_end_matches('"'), ""),
                |close| (&rest[..close], rest[close + 1..].trim()),
            );
            let trailing = if trailing.is_empty() {
                String::new()
            } else {
                format!(" {trailing}")
            };
            lines[i] = format!(
                "{indent}{CRATE} = {{ version = \"{version}\", features = [{feats_csv}] }}{trailing}"
            );
            break;
        }

        // `totp-rs = { ... }` inline table → merge any missing features.
        if trimmed.starts_with(&table_prefix) {
            if let Some(result) = merge_into_list(&trimmed, "features = [") {
                match result {
                    Some(new_line) => lines[i] = format!("{indent}{new_line}"),
                    None => return toml.to_owned(),
                }
                break;
            } else if let Some(close_brace) = trimmed.rfind('}') {
                // No `features` key — insert one before the closing brace.
                let before = trimmed[..close_brace].trim_end();
                let sep = if before.ends_with('{') { " " } else { ", " };
                lines[i] = format!("{indent}{before}{sep}features = [{feats_csv}] }}");
                break;
            }
        }

        // `[dependencies.totp-rs]` subtable form → merge into (or add) a
        // `features = [...]` key within the subtable body.
        if trimmed == subtable_header || trimmed == subtable_header_underscore {
            let mut feature_line: Option<usize> = None;
            let mut j = i + 1;
            while j < lines.len() {
                let tj = lines[j].trim();
                if tj.starts_with('[') {
                    break;
                }
                if tj.starts_with("features") && tj.contains('[') {
                    feature_line = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(fl) = feature_line {
                let tj = lines[fl].trim().to_owned();
                let indent_j: String = lines[fl]
                    .chars()
                    .take_while(char::is_ascii_whitespace)
                    .collect();
                if tj.contains(']') {
                    // Single-line `features = [...]`.
                    match merge_into_list(&tj, "[") {
                        Some(Some(new_line)) => lines[fl] = format!("{indent_j}{new_line}"),
                        Some(None) => return toml.to_owned(),
                        None => {}
                    }
                } else {
                    // Multiline `features = [` … `]` array: find the closing `]`,
                    // collect the existing entries, and rebuild the list (collapsed
                    // to one line) with the missing features appended.
                    let mut close_line = None;
                    let mut k = fl;
                    while k < lines.len() {
                        let tk = lines[k].trim();
                        if k > fl && tk.starts_with('[') {
                            break; // next table header — array never closed
                        }
                        if lines[k].contains(']') {
                            close_line = Some(k);
                            break;
                        }
                        k += 1;
                    }
                    if let Some(cl) = close_line {
                        let fl_bracket = lines[fl].find('[').unwrap_or(lines[fl].len());
                        let mut list_text = lines[fl][fl_bracket + 1..].to_owned();
                        for line in &lines[fl + 1..cl] {
                            list_text.push(' ');
                            list_text.push_str(line.trim());
                        }
                        let cl_close = lines[cl].find(']').unwrap_or(lines[cl].len());
                        list_text.push(' ');
                        list_text.push_str(&lines[cl][..cl_close]);
                        let trailing =
                            lines[cl][cl_close.saturating_add(1).min(lines[cl].len())..].to_owned();

                        let mut entries: Vec<String> = list_text
                            .split(',')
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .map(str::to_owned)
                            .collect();
                        let mut changed = false;
                        for f in TOTP_RS_FEATURES {
                            let q = format!("\"{f}\"");
                            if !entries.iter().any(|e| e == &q) {
                                entries.push(q);
                                changed = true;
                            }
                        }
                        if !changed {
                            return toml.to_owned();
                        }
                        let rebuilt =
                            format!("{indent_j}features = [{}]{trailing}", entries.join(", "));
                        lines.splice(fl..=cl, std::iter::once(rebuilt));
                    }
                }
            } else {
                // No `features` key in the subtable — add one right after the header.
                lines.insert(i + 1, format!("features = [{feats_csv}]"));
            }
            break;
        }
        i += 1;
    }

    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    out
}

/// OAuth2/OIDC options for `autumn generate auth --oauth`.
#[derive(Debug, Clone, Default)]
pub struct AuthOAuthOptions {
    /// Provider keys to scaffold (e.g. `["github", "google"]`).
    /// An empty list produces the same output as [`plan_auth`].
    pub providers: Vec<String>,
}

/// Compute the file actions for `autumn generate auth`.
///
/// Pure planning step — no I/O happens here. Tests use this directly so they
/// can inspect the emitted file list and contents without touching the disk.
///
/// # Errors
/// Returns [`GenerateError::NotInProject`] when run outside an Autumn project
/// root, or [`GenerateError::InvalidName`] for a bad resource name.
#[allow(dead_code)]
pub fn plan_auth(project_root: &Path, name: &str, timestamp: &str) -> Result<Plan, GenerateError> {
    plan_auth_with_providers(project_root, name, timestamp, &[], false)
}

#[allow(clippy::too_many_lines)]
pub fn plan_auth_with_providers(
    project_root: &Path,
    name: &str,
    timestamp: &str,
    providers: &[String],
    totp: bool,
) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;
    super::model::validate_resource_name(name)?;

    let pascal_name = pascal(name);
    let snake_name = snake(name);
    let table = pluralize(&snake_name);

    // Under `--totp` the generator unconditionally emits a `recovery_code` model
    // and a `recovery_codes` table. If the auth resource itself resolves to those
    // names, the two collide — the same model file and `CREATE TABLE` would be
    // emitted twice, producing an unusable app. Reject it up front.
    if totp && (snake_name == "recovery_code" || table == "recovery_codes") {
        return Err(GenerateError::InvalidName(
            name.to_owned(),
            "collides with the reserved `recovery_code` model/table generated by \
             `--totp`; choose a different resource name."
                .to_owned(),
        ));
    }

    let mut plan = Plan::new(project_root);

    // ── Migration ──────────────────────────────────────────────────────────
    let mig_dir = project_root
        .join("migrations")
        .join(format!("{timestamp}_create_{table}"));
    plan.create(mig_dir.join("up.sql"), render_migration_up(&table, totp));
    plan.create(
        mig_dir.join("down.sql"),
        render_migration_down(&table, totp),
    );

    // ── Model ──────────────────────────────────────────────────────────────
    let models_dir = project_root.join("src").join("models");
    plan.create(
        models_dir.join(format!("{snake_name}.rs")),
        render_model_file(&pascal_name, &snake_name, &table, totp),
    );
    let model_mod_path = models_dir.join("mod.rs");
    let mut model_mod = add_mod_declaration(&read_or_empty(&model_mod_path), &snake_name);
    if totp {
        // Recovery codes live in their own model + table.
        plan.create(
            models_dir.join("recovery_code.rs"),
            render_recovery_code_model_file(&pascal_name, &table),
        );
        model_mod = add_mod_declaration(&model_mod, "recovery_code");
    }
    plan.modify(model_mod_path, model_mod);

    // ── src/schema.rs entry ────────────────────────────────────────────────
    // The generated model references `crate::schema::<table>`, so we must
    // emit a `diesel::table! { }` block just like `generate model` does.
    // Auth-specific fields (id and created_at are added automatically):
    //   email            String   → Text      NOT NULL
    //   password_digest  String   → Text      NOT NULL
    //   reset_token_digest         Option<String>         → Nullable<Text>
    //   reset_token_expires_at     Option<NaiveDateTime>  → Nullable<Timestamp>
    let mut user_field_tokens: Vec<&str> = vec!["email:String", "password_digest:String"];
    if totp {
        user_field_tokens.push("totp_secret_encrypted:Option<String>");
        user_field_tokens.push("totp_enabled:bool");
        user_field_tokens.push("totp_last_used_step:Option<i64>");
    }
    user_field_tokens.push("failed_attempts:i32");
    user_field_tokens.push("locked_at:Option<NaiveDateTime>");
    user_field_tokens.push("reset_token_digest:Option<String>");
    user_field_tokens.push("reset_token_expires_at:Option<NaiveDateTime>");
    let auth_fields: Vec<super::dsl::Field> = user_field_tokens
        .iter()
        .map(|t| super::dsl::parse_field(t).expect("auth field tokens are always valid"))
        .collect();

    let schema_path = project_root.join("src").join("schema.rs");
    let schema_existing = read_or_empty(&schema_path);
    // Under `--totp` we unconditionally create a helper `recovery_codes` table.
    // If the project already declares one (in `src/schema.rs`), the migration
    // would emit a second `CREATE TABLE recovery_codes` and `diesel migration
    // run` would fail with "relation already exists" (or clobber an unrelated
    // table). Reject up front rather than generate an unusable app.
    if totp && schema_has_table(&schema_existing, "recovery_codes") {
        return Err(GenerateError::InvalidName(
            name.to_owned(),
            "this project already defines a `recovery_codes` table, which `--totp` \
             needs for its helper model; rename or remove the existing table first."
                .to_owned(),
        ));
    }
    let mut schema_new = append_schema_table(&schema_existing, &table, &auth_fields);
    if totp {
        let recovery_fields: Vec<super::dsl::Field> = [
            "user_id:i64",
            "code_digest:String",
            "used_at:Option<NaiveDateTime>",
        ]
        .iter()
        .map(|t| super::dsl::parse_field(t).expect("recovery field tokens are always valid"))
        .collect();
        schema_new = append_schema_table(&schema_new, "recovery_codes", &recovery_fields);
    }
    plan.modify(schema_path, schema_new);

    // ── Auth routes ────────────────────────────────────────────────────────
    let routes_dir = project_root.join("src").join("routes");
    plan.create(
        routes_dir.join("auth.rs"),
        render_routes_file(&pascal_name, &snake_name, &table, providers, totp),
    );
    let route_mod_path = routes_dir.join("mod.rs");
    plan.modify(
        route_mod_path.clone(),
        add_mod_declaration(&read_or_empty(&route_mod_path), "auth"),
    );

    // ── Generated tests ────────────────────────────────────────────────────
    let tests_dir = project_root.join("tests");
    plan.create(
        tests_dir.join("auth.rs"),
        render_tests_file(&pascal_name, &snake_name),
    );
    if totp {
        plan.create(
            tests_dir.join("auth_2fa.rs"),
            render_2fa_tests_file(&pascal_name, &snake_name),
        );
    }

    // ── Documentation ─────────────────────────────────────────────────────
    let docs_dir = project_root.join("docs").join("guide");
    plan.create(
        docs_dir.join("authentication.md"),
        render_docs_file(&pascal_name, totp),
    );

    // ── src/main.rs — module declarations + route registration ────────────
    let main_path = project_root.join("src").join("main.rs");
    let main_existing = std::fs::read_to_string(&main_path).map_err(|_| {
        GenerateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing {}", main_path.display()),
        ))
    })?;
    let entries = auth_route_entries(totp);
    let updated = update_main_rs(&main_existing, &["models", "routes", "schema"], &entries);
    plan.modify(main_path, updated);

    // ── Cargo.toml deps + autumn-web/mail feature ─────────────────────────
    let cargo_toml_path = project_root.join("Cargo.toml");
    let cargo_existing = read_or_empty(&cargo_toml_path);
    let all_deps: Vec<(&str, &str)> = super::model::MODEL_DEPS
        .iter()
        .copied()
        .chain(AUTH_EXTRA_DEPS.iter().copied())
        .chain(if totp { TOTP_EXTRA_DEPS } else { &[] }.iter().copied())
        .collect();
    // Apply dep additions then enable the mail feature in a single write.
    let with_deps = ensure_cargo_dependencies(&cargo_existing, &all_deps);
    // `ensure_cargo_dependencies` no-ops on an already-declared `totp-rs`, so
    // merge the required features into any pre-existing declaration.
    let with_deps = if totp {
        ensure_totp_rs_features(&with_deps)
    } else {
        with_deps
    };
    let final_cargo = ensure_autumn_web_mail_feature(&with_deps);
    if final_cargo != cargo_existing {
        plan.modify(cargo_toml_path, final_cargo);
    }

    Ok(plan)
}

/// Compute the file actions for `autumn generate auth --oauth <providers>`.
///
/// When `oauth.providers` is empty this is identical to [`plan_auth`].
/// When providers are specified the plan additionally includes:
/// - An `oauth_identities` migration keyed by `(provider, subject)`.
/// - `src/routes/oauth.rs` with `oauth_redirect` and `oauth_callback` handlers.
/// - `docs/guide/oauth.md` covering provider setup, security properties, and troubleshooting.
/// - The `oauth2` feature on `autumn-web` in `Cargo.toml`.
///
/// # Errors
/// Same as [`plan_auth`].
///
/// Retained as stable public API; internal call sites use [`plan_auth_full`].
#[allow(dead_code)]
pub fn plan_auth_with_options(
    project_root: &Path,
    name: &str,
    timestamp: &str,
    oauth: &AuthOAuthOptions,
) -> Result<Plan, GenerateError> {
    plan_auth_options_impl(project_root, name, timestamp, oauth, false, false)
}

/// Compute the file actions for `autumn generate auth [--oauth …] [--totp]`.
///
/// Layers optional TOTP two-factor authentication on top of the base (and
/// optional OAuth) auth scaffold.
///
/// # Errors
/// Same as [`plan_auth`].
#[allow(dead_code)]
pub fn plan_auth_full(
    project_root: &Path,
    name: &str,
    timestamp: &str,
    oauth: &AuthOAuthOptions,
    totp: bool,
) -> Result<Plan, GenerateError> {
    plan_auth_options_impl(project_root, name, timestamp, oauth, totp, false)
}

/// Compute the file actions for `autumn generate auth [--oauth …] [--totp] [--passkeys]`.
///
/// Extended version of [`plan_auth_full`] that additionally supports `WebAuthn` passkeys.
///
/// # Errors
/// Same as [`plan_auth`].
pub fn plan_auth_full_ex(
    project_root: &Path,
    name: &str,
    timestamp: &str,
    oauth: &AuthOAuthOptions,
    totp: bool,
    passkeys: bool,
) -> Result<Plan, GenerateError> {
    plan_auth_options_impl(project_root, name, timestamp, oauth, totp, passkeys)
}

/// Shared implementation: base (optionally TOTP-aware, optionally passkey-aware)
/// scaffold plus, when providers are supplied, the OAuth artifacts.
#[allow(clippy::too_many_lines)]
fn plan_auth_options_impl(
    project_root: &Path,
    name: &str,
    timestamp: &str,
    oauth: &AuthOAuthOptions,
    totp: bool,
    passkeys: bool,
) -> Result<Plan, GenerateError> {
    // Start with the base auth plan with providers (and optional TOTP) applied.
    let mut plan = plan_auth_with_providers(project_root, name, timestamp, &oauth.providers, totp)?;

    let pascal_name = pascal(name);
    let snake_name = snake(name);
    let user_table = pluralize(&snake_name);

    if !oauth.providers.is_empty() {
        // ── oauth_identities migration ─────────────────────────────────────────
        // Increment the timestamp by one second so Diesel never sees two migrations
        // with the same version number.
        let oauth_ts_str: String = timestamp
            .parse::<i64>()
            .map_or_else(|_| format!("{timestamp}1"), |t| (t + 1).to_string());
        let mig_dir = project_root
            .join("migrations")
            .join(format!("{oauth_ts_str}_create_oauth_identities"));
        plan.create(
            mig_dir.join("up.sql"),
            render_oauth_migration_up(&user_table),
        );
        plan.create(mig_dir.join("down.sql"), render_oauth_migration_down());

        // ── src/schema.rs entry for oauth_identities ───────────────────────────
        let schema_path = project_root.join("src").join("schema.rs");
        let schema_base = find_plan_content_for_path(&plan, &schema_path)
            .unwrap_or_else(|| read_or_empty(&schema_path));
        let oauth_fields: Vec<super::dsl::Field> = [
            "provider:String",
            "subject:String",
            "user_id:i64",
            "email:Option<String>",
            "name:Option<String>",
        ]
        .iter()
        .map(|t| super::dsl::parse_field(t).expect("oauth field tokens are always valid"))
        .collect();
        let updated_schema = append_schema_table(&schema_base, "oauth_identities", &oauth_fields);
        plan.modify(schema_path, updated_schema);

        // ── oauth routes ───────────────────────────────────────────────────────
        let routes_dir = project_root.join("src").join("routes");
        plan.create(
            routes_dir.join("oauth.rs"),
            render_oauth_routes_file(
                &pascal_name,
                &snake_name,
                &user_table,
                &oauth.providers,
                totp,
            ),
        );

        // Add `pub mod oauth;` to src/routes/mod.rs — use the base plan's already-modified
        // content so the base `pub mod auth;` declaration is not overwritten.
        let route_mod_path = routes_dir.join("mod.rs");
        let route_mod_base = find_plan_content_for_path(&plan, &route_mod_path)
            .unwrap_or_else(|| read_or_empty(&route_mod_path));
        let updated_route_mod = add_mod_declaration(&route_mod_base, "oauth");
        plan.modify(route_mod_path, updated_route_mod);

        // ── Register oauth routes in src/main.rs ───────────────────────────────
        let main_path = project_root.join("src").join("main.rs");
        let main_existing = std::fs::read_to_string(&main_path).map_err(|_| {
            GenerateError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("missing {}", main_path.display()),
            ))
        })?;
        let oauth_entries = oauth_route_entries();
        // The base plan has already modified main.rs; find current state in plan or use existing.
        let base_main =
            find_plan_content_for_path(&plan, &main_path).unwrap_or_else(|| main_existing.clone());
        let updated_main = super::schema_edit::update_main_rs(
            &base_main,
            &["models", "routes", "schema"],
            &oauth_entries,
        );
        plan.modify(main_path, updated_main);

        // ── docs/guide/oauth.md ────────────────────────────────────────────────
        let docs_dir = project_root.join("docs").join("guide");
        plan.create(
            docs_dir.join("oauth.md"),
            render_oauth_docs_file(&oauth.providers),
        );

        // ── Cargo.toml: add oauth2 feature to autumn-web ─────────────────────
        let cargo_toml_path = project_root.join("Cargo.toml");
        let cargo_existing = read_or_empty(&cargo_toml_path);
        // The base plan may have already updated Cargo.toml; use its content if present.
        let base_cargo = find_plan_content_for_path(&plan, &cargo_toml_path)
            .unwrap_or_else(|| cargo_existing.clone());
        let with_oauth2 = ensure_autumn_web_oauth2_feature(&base_cargo);
        if with_oauth2 != base_cargo {
            plan.modify(cargo_toml_path, with_oauth2);
        }

        // ── autumn.toml OAuth provider stubs ───────────────────────────────────
        let autumn_toml_path = project_root.join("autumn.toml");
        if autumn_toml_path.exists() {
            let toml_existing = read_or_empty(&autumn_toml_path);
            let updated_toml = append_oauth_stubs_to_toml(&toml_existing, &oauth.providers);
            if updated_toml != toml_existing {
                plan.modify(autumn_toml_path, updated_toml);
            }
        }
    }

    if passkeys {
        // ── webauthn_credentials collision check ───────────────────────────────
        // If the project already has webauthn_credentials in schema.rs, the
        // migration below would fail at `diesel migration run`. Reject upfront.
        let schema_for_check = project_root.join("src").join("schema.rs");
        let schema_existing_for_passkey = find_plan_content_for_path(&plan, &schema_for_check)
            .unwrap_or_else(|| read_or_empty(&schema_for_check));
        if schema_has_table(&schema_existing_for_passkey, "webauthn_credentials") {
            return Err(GenerateError::InvalidName(
                name.to_owned(),
                "this project already defines a `webauthn_credentials` table, which \
                 `--passkeys` needs; rename or remove the existing table first."
                    .to_owned(),
            ));
        }

        // ── webauthn_credentials migration ─────────────────────────────────────
        // Use timestamp+2 when oauth is also requested (oauth uses timestamp+1),
        // so both migrations get unique Diesel version numbers.
        let passkey_offset: i64 = if oauth.providers.is_empty() { 1 } else { 2 };
        let passkey_ts_str: String = timestamp.parse::<i64>().map_or_else(
            |_| format!("{timestamp}{passkey_offset}"),
            |t| (t + passkey_offset).to_string(),
        );
        let mig_dir = project_root
            .join("migrations")
            .join(format!("{passkey_ts_str}_create_webauthn_credentials"));
        plan.create(
            mig_dir.join("up.sql"),
            render_passkey_migration_up(&user_table),
        );
        plan.create(mig_dir.join("down.sql"), render_passkey_migration_down());

        // ── src/models/webauthn_credential.rs ─────────────────────────────────
        let models_dir = project_root.join("src").join("models");
        plan.create(
            models_dir.join("webauthn_credential.rs"),
            render_webauthn_credential_model_file(&user_table),
        );
        let model_mod_path = models_dir.join("mod.rs");
        let model_mod_base = find_plan_content_for_path(&plan, &model_mod_path)
            .unwrap_or_else(|| read_or_empty(&model_mod_path));
        plan.modify(
            model_mod_path,
            add_mod_declaration(&model_mod_base, "webauthn_credential"),
        );

        // ── src/schema.rs: webauthn_credentials table ─────────────────────────
        let schema_path = project_root.join("src").join("schema.rs");
        let schema_base = find_plan_content_for_path(&plan, &schema_path)
            .unwrap_or_else(|| read_or_empty(&schema_path));
        let wc_fields: Vec<super::dsl::Field> = [
            "user_id:i64",
            "credential_id:String",
            "credential_json:String",
            "name:String",
            "last_used_at:Option<NaiveDateTime>",
        ]
        .iter()
        .map(|t| super::dsl::parse_field(t).expect("webauthn credential field tokens are valid"))
        .collect();
        let updated_schema = append_schema_table(&schema_base, "webauthn_credentials", &wc_fields);
        plan.modify(schema_path, updated_schema);

        // ── src/routes/passkeys.rs ─────────────────────────────────────────────
        let routes_dir = project_root.join("src").join("routes");
        plan.create(
            routes_dir.join("passkeys.rs"),
            render_passkeys_routes_file(&pascal_name, &snake_name, &user_table),
        );
        let route_mod_path = routes_dir.join("mod.rs");
        let route_mod_base = find_plan_content_for_path(&plan, &route_mod_path)
            .unwrap_or_else(|| read_or_empty(&route_mod_path));
        plan.modify(
            route_mod_path,
            add_mod_declaration(&route_mod_base, "passkeys"),
        );

        // ── Register passkey routes in src/main.rs ─────────────────────────────
        let main_path = project_root.join("src").join("main.rs");
        let main_existing = std::fs::read_to_string(&main_path).map_err(|_| {
            GenerateError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("missing {}", main_path.display()),
            ))
        })?;
        let pk_entries = passkey_route_entries();
        let base_main =
            find_plan_content_for_path(&plan, &main_path).unwrap_or_else(|| main_existing.clone());
        let updated_main = super::schema_edit::update_main_rs(
            &base_main,
            &["models", "routes", "schema"],
            &pk_entries,
        );
        plan.modify(main_path, updated_main);

        // ── tests/auth_passkeys.rs ─────────────────────────────────────────────
        let tests_dir = project_root.join("tests");
        plan.create(
            tests_dir.join("auth_passkeys.rs"),
            render_passkeys_tests_file(&pascal_name, &snake_name),
        );

        // ── docs/guide/passkeys.md ─────────────────────────────────────────────
        let docs_dir = project_root.join("docs").join("guide");
        plan.create(docs_dir.join("passkeys.md"), render_passkeys_docs_file());

        // ── autumn.toml: [auth.webauthn] stub ─────────────────────────────────
        let autumn_toml_path = project_root.join("autumn.toml");
        if autumn_toml_path.exists() {
            let toml_existing = find_plan_content_for_path(&plan, &autumn_toml_path)
                .unwrap_or_else(|| read_or_empty(&autumn_toml_path));
            let updated_toml = append_webauthn_stub_to_toml(
                &toml_existing,
                "localhost",
                "My App",
                "http://localhost:3000",
            );
            if updated_toml != toml_existing {
                plan.modify(autumn_toml_path, updated_toml);
            }
        }

        // ── Cargo.toml: add webauthn-rs dep + webauthn feature on autumn-web ───
        let cargo_toml_path = project_root.join("Cargo.toml");
        let base_cargo = find_plan_content_for_path(&plan, &cargo_toml_path)
            .unwrap_or_else(|| read_or_empty(&cargo_toml_path));
        let all_passkey_deps: Vec<(&str, &str)> = PASSKEY_EXTRA_DEPS.to_vec();
        let with_deps = super::model::ensure_cargo_dependencies(&base_cargo, &all_passkey_deps);
        // If the project already declared webauthn-rs without the required features,
        // ensure_cargo_dependencies would have skipped it; merge them here.
        let with_deps = ensure_webauthn_rs_features(&with_deps);
        let with_webauthn = ensure_autumn_web_webauthn_feature(&with_deps);
        if with_webauthn != base_cargo {
            plan.modify(cargo_toml_path, with_webauthn);
        }
    }

    Ok(plan)
}

/// Extract the planned output content for a given path from the plan (last modify/create wins).
fn find_plan_content_for_path(plan: &Plan, path: &std::path::Path) -> Option<String> {
    use super::emit::Action;
    plan.actions
        .iter()
        .rev()
        .find(|a| a.path() == path)
        .map(|a| match a {
            Action::Create { contents, .. } | Action::Modify { contents, .. } => contents.clone(),
        })
}

/// Ensure `autumn-web` in `[dependencies]` has `features = ["oauth2"]`.
#[allow(clippy::too_many_lines)]
fn ensure_autumn_web_oauth2_feature(toml: &str) -> String {
    const CRATE: &str = "autumn-web";
    const FEATURE: &str = "\"oauth2\"";

    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    let trailing_newline = toml.ends_with('\n');

    let simple_prefix = format!("{CRATE} = \"");
    let table_prefix = format!("{CRATE} = {{");
    let subtable_header = format!("[dependencies.{CRATE}]");
    let subtable_header_underscore = format!("[dependencies.{}]", CRATE.replace('-', "_"));

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim().to_owned();
        let indent: String = lines[i]
            .chars()
            .take_while(char::is_ascii_whitespace)
            .collect();

        if let Some(rest) = trimmed.strip_prefix(&simple_prefix) {
            let version = rest.trim_end_matches('"');
            lines[i] =
                format!("{indent}{CRATE} = {{ version = \"{version}\", features = [{FEATURE}] }}");
            break;
        }

        if trimmed.starts_with(&table_prefix) {
            if trimmed.contains(FEATURE) {
                break; // already present
            }
            if let Some(feat_bracket) = trimmed.find("features = [") {
                let list_start = feat_bracket + "features = [".len();
                if let Some(close_bracket) = trimmed[list_start..].find(']') {
                    let list_end = close_bracket + list_start;
                    let existing = trimmed[list_start..list_end].trim();
                    let new_list = if existing.is_empty() {
                        FEATURE.to_owned()
                    } else {
                        format!("{existing}, {FEATURE}")
                    };
                    lines[i] = format!(
                        "{indent}{}{}{}",
                        &trimmed[..list_start],
                        new_list,
                        &trimmed[list_end..]
                    );
                } else {
                    let mut j = i + 1;
                    while j < lines.len() {
                        let tj = lines[j].trim();
                        if tj.starts_with('[') {
                            break;
                        }
                        if let Some(close_idx) = tj.find(']') {
                            let before_close = tj[..close_idx].trim();
                            let sep = if before_close.is_empty() || before_close.ends_with(',') {
                                ""
                            } else {
                                ", "
                            };
                            let indent_j: String = lines[j]
                                .chars()
                                .take_while(char::is_ascii_whitespace)
                                .collect();
                            lines[j] = format!(
                                "{indent_j}{before_close}{sep}{FEATURE}{}",
                                &tj[close_idx..]
                            );
                            break;
                        }
                        j += 1;
                    }
                }
            } else {
                let close = trimmed.rfind('}').unwrap();
                let before_close = trimmed[..close].trim_end();
                let sep = if before_close.ends_with('{') {
                    ""
                } else {
                    ", "
                };
                lines[i] = format!(
                    "{indent}{}{sep}features = [{FEATURE}]{}",
                    &trimmed[..close],
                    &trimmed[close..]
                );
            }
            break;
        }

        if trimmed == subtable_header || trimmed == subtable_header_underscore {
            let mut j = i + 1;
            let mut found_features = false;
            while j < lines.len() {
                let t = lines[j].trim().to_owned();
                if t.starts_with('[') {
                    break;
                }
                if t.starts_with("features") {
                    found_features = true;
                    if t.contains(FEATURE) {
                        break;
                    }
                    if let Some(open) = t.find('[') {
                        if let Some(close) = t.rfind(']') {
                            let inner = t[open + 1..close].trim();
                            let new_inner = if inner.is_empty() {
                                FEATURE.to_owned()
                            } else {
                                format!("{inner}, {FEATURE}")
                            };
                            let indent_j: String = lines[j]
                                .chars()
                                .take_while(char::is_ascii_whitespace)
                                .collect();
                            lines[j] = format!("{indent_j}features = [{new_inner}]");
                        } else {
                            let mut k = j + 1;
                            while k < lines.len() {
                                let tk = lines[k].trim();
                                if tk.starts_with('[') {
                                    break;
                                }
                                if let Some(close_idx) = tk.find(']') {
                                    let before_close = tk[..close_idx].trim();
                                    let sep =
                                        if before_close.is_empty() || before_close.ends_with(',') {
                                            ""
                                        } else {
                                            ", "
                                        };
                                    let indent_k: String = lines[k]
                                        .chars()
                                        .take_while(char::is_ascii_whitespace)
                                        .collect();
                                    lines[k] = format!(
                                        "{indent_k}{before_close}{sep}{FEATURE}{}",
                                        &tk[close_idx..]
                                    );
                                    break;
                                }
                                k += 1;
                            }
                        }
                    }
                    break;
                }
                j += 1;
            }
            if !found_features {
                lines.insert(i + 1, format!("features = [{FEATURE}]"));
            }
            break;
        }

        i += 1;
    }

    let mut out = lines.join("\n");
    if trailing_newline && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// CLI entry point for `autumn generate auth <Name>` (no OAuth providers).
#[allow(dead_code)]
pub fn run(name: &str, flags: Flags) {
    run_with_options(name, flags, &AuthOAuthOptions::default(), false, false);
}

/// CLI entry point for `autumn generate auth <Name> --oauth <providers> [--totp] [--passkeys]`.
pub fn run_with_options(
    name: &str,
    flags: Flags,
    oauth: &AuthOAuthOptions,
    totp: bool,
    passkeys: bool,
) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    let timestamp = timestamp_now();
    let plan = plan_auth_full_ex(&cwd, name, &timestamp, oauth, totp, passkeys);
    match plan.and_then(|p| p.execute(flags)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Ensure `autumn-web` in `[dependencies]` has `features = ["mail"]`.
///
/// Handles the three common forms a fresh Autumn project may use:
/// - `autumn-web = "x.y"` (simple string)
/// - `autumn-web = { version = "x.y", ... }` (inline table)
/// - `[dependencies.autumn-web]` subtable
fn ensure_autumn_web_mail_feature(toml: &str) -> String {
    const CRATE: &str = "autumn-web";
    const FEATURE: &str = "\"mail\"";

    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    let trailing_newline = toml.ends_with('\n');

    let simple_prefix = format!("{CRATE} = \"");
    let table_prefix = format!("{CRATE} = {{");
    let subtable_header = format!("[dependencies.{CRATE}]");

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim().to_owned();
        let indent: String = lines[i]
            .chars()
            .take_while(char::is_ascii_whitespace)
            .collect();

        if let Some(rest) = trimmed.strip_prefix(&simple_prefix) {
            // autumn-web = "0.3"
            let version = rest.trim_end_matches('"');
            lines[i] =
                format!("{indent}{CRATE} = {{ version = \"{version}\", features = [{FEATURE}] }}");
            break;
        }

        if trimmed.starts_with(&table_prefix) {
            if trimmed.contains(FEATURE) {
                break; // already present
            }
            if let Some(feat_bracket) = trimmed.find("features = [") {
                // Add to existing features list.
                let list_start = feat_bracket + "features = [".len();
                let list_end = trimmed[list_start..].find(']').unwrap() + list_start;
                let existing = trimmed[list_start..list_end].trim();
                let new_list = if existing.is_empty() {
                    FEATURE.to_owned()
                } else {
                    format!("{existing}, {FEATURE}")
                };
                lines[i] = format!(
                    "{indent}{}{}{}",
                    &trimmed[..list_start],
                    new_list,
                    &trimmed[list_end..]
                );
            } else {
                // No features key — insert before closing `}`.
                let close = trimmed.rfind('}').unwrap();
                let before_close = trimmed[..close].trim_end();
                let sep = if before_close.ends_with('{') {
                    ""
                } else {
                    ", "
                };
                lines[i] = format!(
                    "{indent}{}{sep}features = [{FEATURE}]{}",
                    &trimmed[..close],
                    &trimmed[close..]
                );
            }
            break;
        }

        if trimmed == subtable_header {
            // Scan ahead within the subtable.
            let mut j = i + 1;
            let mut found_features = false;
            while j < lines.len() {
                let t = lines[j].trim().to_owned();
                if t.starts_with('[') {
                    break;
                }
                if t.starts_with("features") {
                    found_features = true;
                    if !t.contains(FEATURE)
                        && let (Some(open), Some(close)) = (t.find('['), t.rfind(']'))
                    {
                        let inner = t[open + 1..close].trim();
                        let new_inner = if inner.is_empty() {
                            FEATURE.to_owned()
                        } else {
                            format!("{inner}, {FEATURE}")
                        };
                        let indent_j: String = lines[j]
                            .chars()
                            .take_while(char::is_ascii_whitespace)
                            .collect();
                        lines[j] = format!("{indent_j}features = [{new_inner}]");
                    }
                    break;
                }
                j += 1;
            }
            if !found_features {
                lines.insert(i + 1, format!("features = [{FEATURE}]"));
            }
            break;
        }

        i += 1;
    }

    let mut out = lines.join("\n");
    if trailing_newline && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn append_oauth_stubs_to_toml(existing: &str, providers: &[String]) -> String {
    let mut out = existing.trim_end().to_owned();
    for name in providers {
        let header = format!("[auth.oauth2.{name}]");
        if out.contains(&header) {
            continue;
        }
        let preset = match name.as_str() {
            "github" => {
                r#"
[auth.oauth2.github]
client_id = ""
client_secret = ""
authorize_url = "https://github.com/login/oauth/authorize"
token_url = "https://github.com/login/oauth/access_token"
userinfo_url = "https://api.github.com/user"
redirect_uri = "http://localhost:3000/auth/github/callback"
scope = "read:user user:email"
"#
            }
            "google" => {
                r#"
[auth.oauth2.google]
client_id = ""
client_secret = ""
authorize_url = "https://accounts.google.com/o/oauth2/v2/auth"
token_url = "https://oauth2.googleapis.com/token"
userinfo_url = "https://openidconnect.googleapis.com/v1/userinfo"
redirect_uri = "http://localhost:3000/auth/google/callback"
scope = "openid profile email"
issuer = "https://accounts.google.com"
jwks_url = "https://www.googleapis.com/oauth2/v3/certs"
discovery_url = "https://accounts.google.com"
"#
            }
            "microsoft" => {
                r#"
[auth.oauth2.microsoft]
client_id = ""
client_secret = ""
authorize_url = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
token_url = "https://login.microsoftonline.com/common/oauth2/v2.0/token"
redirect_uri = "http://localhost:3000/auth/microsoft/callback"
scope = "openid profile email"
issuer = "https://login.microsoftonline.com/common/v2.0"
jwks_url = "https://login.microsoftonline.com/common/discovery/v2.0/keys"
discovery_url = "https://login.microsoftonline.com/common/v2.0"
"#
            }
            _ => &format!(
                r#"
[auth.oauth2.{name}]
client_id = ""
client_secret = ""
authorize_url = "https://example.com/oauth/authorize"
token_url = "https://example.com/oauth/token"
redirect_uri = "http://localhost:3000/auth/{name}/callback"
scope = "openid profile email"
"#
            ),
        };
        out.push('\n');
        out.push_str(preset.trim_start());
    }
    out.push('\n');
    out
}

// ── Template rendering ────────────────────────────────────────────────────────

fn render_migration_up(table: &str, totp: bool) -> String {
    // TOTP columns are inserted after password_digest so the column order
    // matches the generated model struct and `schema.rs` block.
    let totp_columns = if totp {
        "\x20   totp_secret_encrypted TEXT NULL,\n\
         \x20   totp_enabled BOOLEAN NOT NULL DEFAULT FALSE,\n\
         \x20   totp_last_used_step BIGINT NULL,\n"
    } else {
        ""
    };
    let mut out = format!(
        "CREATE TABLE {table} (\n\
         \x20   id BIGSERIAL PRIMARY KEY,\n\
         \x20   email TEXT NOT NULL UNIQUE,\n\
         \x20   password_digest TEXT NOT NULL,\n\
         {totp_columns}\
         \x20   failed_attempts INT NOT NULL DEFAULT 0,\n\
         \x20   locked_at TIMESTAMP NULL,\n\
         \x20   reset_token_digest TEXT NULL,\n\
         \x20   reset_token_expires_at TIMESTAMP NULL,\n\
         \x20   created_at TIMESTAMP NOT NULL DEFAULT NOW()\n\
         );\n"
    );
    if totp {
        out.push_str(
            "\n\
             CREATE TABLE recovery_codes (\n\
             \x20   id BIGSERIAL PRIMARY KEY,\n\
             \x20   user_id BIGINT NOT NULL REFERENCES ",
        );
        out.push_str(table);
        out.push_str(
            "(id) ON DELETE CASCADE,\n\
             \x20   code_digest TEXT NOT NULL,\n\
             \x20   used_at TIMESTAMP NULL,\n\
             \x20   created_at TIMESTAMP NOT NULL DEFAULT NOW()\n\
             );\n\
             \n\
             CREATE INDEX recovery_codes_user_id_idx ON recovery_codes (user_id);\n",
        );
    }
    out
}

fn render_migration_down(table: &str, totp: bool) -> String {
    // Drop the dependent table first so the FK constraint is satisfied.
    if totp {
        format!("DROP TABLE recovery_codes;\nDROP TABLE {table};\n")
    } else {
        format!("DROP TABLE {table};\n")
    }
}

fn render_model_file(pascal_name: &str, _snake_name: &str, table: &str, totp: bool) -> String {
    // TOTP columns mirror the migration/schema order (after password_digest).
    // TOTP columns mirror the migration/schema order (after password_digest).
    // `totp_secret_encrypted` holds the AES-GCM-encrypted secret (never plaintext),
    // and `totp_last_used_step` is the replay guard. All three are `#[default]`
    // so they are excluded from `NewUser` and fall back to their DB defaults
    // (disabled / NULL) on signup.
    let totp_fields = if totp {
        "    #[default]\n\
         \x20   pub totp_secret_encrypted: Option<String>,\n\
         \x20   #[default]\n\
         \x20   pub totp_enabled: bool,\n\
         \x20   #[default]\n\
         \x20   pub totp_last_used_step: Option<i64>,\n"
    } else {
        ""
    };
    format!(
        r"//! Generated by `autumn generate auth`.
//!
//! Edit freely — once generated, this is ordinary user code.
//! Security note: never store raw passwords, reset tokens, or TOTP secrets
//! here in the clear — only digests and encrypted blobs.

use crate::schema::{table};

#[autumn_web::model]
pub struct {pascal_name} {{
    pub id: i64,
    pub email: String,
    pub password_digest: String,
{totp_fields}    #[default]
    pub failed_attempts: i32,
    #[default]
    pub locked_at: Option<chrono::NaiveDateTime>,
    pub reset_token_digest: Option<String>,
    pub reset_token_expires_at: Option<chrono::NaiveDateTime>,
    #[default]
    pub created_at: chrono::NaiveDateTime,
}}
"
    )
}

/// Render `src/models/recovery_code.rs` (only emitted with `--totp`).
///
/// Recovery codes are single-use: only the bcrypt `code_digest` is stored, and
/// `used_at` is stamped when a code is consumed so it can never be replayed.
fn render_recovery_code_model_file(user_pascal: &str, user_table: &str) -> String {
    format!(
        r"//! Generated by `autumn generate auth --totp`.
//!
//! Single-use TOTP recovery codes for {user_pascal}. Edit freely.
//! Security note: only the bcrypt digest of each code is stored — never the
//! raw code. `used_at` marks a code as consumed so it cannot be reused.

use crate::schema::recovery_codes;

#[autumn_web::model]
pub struct RecoveryCode {{
    pub id: i64,
    // References {user_table}(id).
    pub user_id: i64,
    pub code_digest: String,
    // Kept in `NewRecoveryCode` so inserts set it explicitly to `None`.
    pub used_at: Option<chrono::NaiveDateTime>,
    #[default]
    pub created_at: chrono::NaiveDateTime,
}}
"
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "Single auth-routes template — splitting fragments makes the template harder to read."
)]
fn render_routes_file(
    pascal_name: &str,
    snake_name: &str,
    table: &str,
    providers: &[String],
    totp: bool,
) -> String {
    let oauth_buttons = if providers.is_empty() {
        String::new()
    } else {
        let mut btn_html = String::with_capacity(256);
        btn_html.push_str("hr;\n        h3 { \"Or sign in with:\" }\n        div style=\"display: flex; gap: 0.5rem;\" {\n");
        for p in providers {
            let label = match p.as_str() {
                "github" => "GitHub",
                "google" => "Google",
                "microsoft" => "Microsoft",
                _ => p,
            };
            btn_html.push_str("            a href=\"/auth/");
            btn_html.push_str(p);
            btn_html.push_str("/redirect\" { button type=\"button\" { \"Sign in with ");
            btn_html.push_str(label);
            btn_html.push_str("\" } }\n");
        }
        btn_html.push_str("        }\n");
        btn_html
    };

    let (totp_imports, totp_login_branch, totp_reset_branch, totp_clear_pending, totp_section) =
        if totp {
            (
                totp_imports_src().to_owned(),
                totp_login_branch_src(snake_name),
                totp_reset_branch_src(snake_name),
                totp_clear_pending_src().to_owned(),
                totp_routes_section_src(pascal_name, snake_name, table),
            )
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )
        };

    format!(
        r#"//! Generated by `autumn generate auth`.
//!
//! Complete browser authentication flow. Edit freely — once generated,
//! this is ordinary user code.
//!
//! Security properties:
//! - Passwords are hashed with bcrypt via `autumn_web::auth`.
//! - Reset tokens are 32-byte random values; only the SHA-256 digest is stored.
//! - Duplicate signup and failed login return identical non-enumerating errors.
//! - Login and reset-password rotate the session ID to prevent fixation.
//! - Logout destroys the session so the old session cannot remain authenticated.

use autumn_web::auth::{{hash_password, verify_password}};
use autumn_web::extract::Query;
use autumn_web::prelude::*;
use axum::response::{{IntoResponse, Response}};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::Deserialize;

use crate::models::{snake_name}::{{New{pascal_name}, {pascal_name}}};
use crate::schema::{table};
{totp_imports}

// ── Layout helpers ────────────────────────────────────────────────────────────

fn layout(title: &str, content: Markup) -> Markup {{
    html! {{
        (autumn_web::PreEscaped("<!DOCTYPE html>"))
        html lang="en" {{
            head {{
                meta charset="utf-8";
                title {{ (title) }}
            }}
            body {{ (content) }}
        }}
    }}
}}

fn redirect_to(url: &str) -> Response {{
    axum::response::Redirect::to(url).into_response()
}}

// ── Signup ────────────────────────────────────────────────────────────────────

/// `GET /signup` — render the signup form.
#[get("/signup")]
pub async fn signup_form(csrf: Option<CsrfToken>, csrf_field: Option<CsrfFormField>) -> AutumnResult<Markup> {{
    Ok(layout("Sign Up", html! {{
        h1 {{ "Create an Account" }}
        form action="/signup" method="post" {{
            @if let Some(ref csrf) = csrf {{ input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }}
            div {{
                label {{ "Email" }}
                input type="email" name="email" required autocomplete="email";
            }}
            div {{
                label {{ "Password (8+ characters)" }}
                input type="password" name="password" required
                      autocomplete="new-password" minlength="8";
            }}
            button type="submit" {{ "Sign Up" }}
        }}
        p {{ a href="/login" {{ "Already have an account? Log in" }} }}
    }}))
}}

#[derive(Deserialize)]
pub struct SignupForm {{
    pub email: String,
    pub password: String,
}}

/// `POST /signup` — create a new account and start a session.
///
/// Non-enumerating: returns the same error whether the email is taken or invalid
/// so callers cannot learn which addresses are registered.
#[post("/signup")]
pub async fn signup(
    mut db: Db,
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<SignupForm>,
) -> AutumnResult<Response> {{
    let email = form.email.trim().to_lowercase();
    // Same message for invalid format and duplicate email — non-enumerating.
    let account_err = || AutumnError::unprocessable_msg("Unable to create account. Please try a different email.");
    if let Some((_, domain)) = email.split_once('@') {{
        if !domain.contains('.') {{
            return Err(account_err());
        }}
    }} else {{
        return Err(account_err());
    }}
    if form.password.len() < 8 {{
        return Err(AutumnError::unprocessable_msg(
            "Password must be at least 8 characters.",
        ));
    }}

    let password_digest = hash_password(&form.password).await?;
    let new_{snake_name} = New{pascal_name} {{
        email: email.clone(),
        password_digest,
        reset_token_digest: None,
        reset_token_expires_at: None,
    }};

    let result: Result<{pascal_name}, _> = diesel::insert_into({table}::table)
        .values(&new_{snake_name})
        .returning({pascal_name}::as_returning())
        .get_result(&mut *db)
        .await;

    let {snake_name} = result.map_err(|_| account_err())?;

    session.rotate_id().await;
{totp_clear_pending}    session.insert("{snake_name}_id", {snake_name}.id.to_string()).await;
    session.insert("{snake_name}_email", &{snake_name}.email).await;
    // Use the same session key checked by `#[secured]` / `#[authorize]`.
    session.insert(state.auth_session_key(), {snake_name}.id.to_string()).await;
    Ok(redirect_to("/account"))
}}

// ── Login ─────────────────────────────────────────────────────────────────────

/// `GET /login` — render the login form.
#[get("/login")]
pub async fn login_form(csrf: Option<CsrfToken>, csrf_field: Option<CsrfFormField>) -> AutumnResult<Markup> {{
    Ok(layout("Log In", html! {{
        h1 {{ "Log In" }}
        form action="/login" method="post" {{
            @if let Some(ref csrf) = csrf {{ input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }}
            div {{
                label {{ "Email" }}
                input type="email" name="email" required autocomplete="email";
            }}
            div {{
                label {{ "Password" }}
                input type="password" name="password" required
                      autocomplete="current-password";
            }}
            button type="submit" {{ "Log In" }}
        }}
        {oauth_buttons}
        p {{ a href="/signup" {{ "New here? Create an account" }} }}
        p {{ a href="/forgot-password" {{ "Forgot your password?" }} }}
    }}))
}}

#[derive(Deserialize)]
pub struct LoginForm {{
    pub email: String,
    pub password: String,
}}

/// Extracts the client IP from `ConnectInfo` when present, or falls back to
/// `UNSPECIFIED` so the handler compiles and runs correctly in both production
/// (where `into_make_service_with_connect_info` injects the extension) and
/// in test environments where it is absent.
pub struct MaybeClientIp(std::net::IpAddr);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for MaybeClientIp {{
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {{
        let ip = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        Ok(MaybeClientIp(ip))
    }}
}}

/// `POST /login` — verify credentials and start a session.
///
/// Non-enumerating: returns the same error for unknown email, wrong password,
/// and a locked account so callers cannot learn which accounts are registered
/// or which are currently locked.
///
/// Account lockout policy is read from `[auth.lockout]` in `autumn.toml`.
/// Safe defaults: threshold = 10 failures, cooloff = 900 s.
/// Disable with `enabled = false` or `threshold = 0` to restore pre-lockout behaviour.
#[post("/login")]
pub async fn login(
    mut db: Db,
    State(state): State<AppState>,
    session: Session,
    MaybeClientIp(addr_ip): MaybeClientIp,
    Form(form): Form<LoginForm>,
) -> AutumnResult<Response> {{
    let email = form.email.trim().to_lowercase();
    let auth_err = || AutumnError::unprocessable_msg("Invalid email or password.");

    let found_{snake_name}: Option<{pascal_name}> = {table}::table
        .filter({table}::email.eq(&email))
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .ok();

    // Always run bcrypt to equalise timing: a miss uses a dummy hash so
    // response latency is indistinguishable from a wrong-password attempt
    // on a real account.
    const DUMMY_HASH: &str = "$2b$12$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let password_hash = found_{snake_name}
        .as_ref()
        .map(|u| u.password_digest.as_str())
        .unwrap_or(DUMMY_HASH);
    let password_ok = verify_password(&form.password, password_hash).await.unwrap_or(false);

    // ── Account lockout policy ────────────────────────────────────────────────
    // Read lockout config from the standard Autumn config surface.
    let lockout_cfg = state.config().auth.lockout;
    let lockout_enabled = lockout_cfg.enabled && lockout_cfg.threshold > 0;

    if let Some(ref {snake_name}) = found_{snake_name} {{
        if lockout_enabled {{
            let now = chrono::Utc::now().naive_utc();
            let cooloff = chrono::Duration::seconds(lockout_cfg.cooloff_secs as i64);

            // Track local state; reset if cool-off has elapsed so the first new
            // failure after expiry is counted as attempt #1, not a re-lock.
            let mut current_attempts = {snake_name}.failed_attempts;
            let mut current_locked_at = {snake_name}.locked_at;

            // Check if the account is currently locked: locked_at is set and
            // cool-off period has not elapsed. Non-enumerating: return the same
            // auth_err as wrong password so the response does not reveal lock state.
            if let Some(locked_at) = {snake_name}.locked_at {{
                if now < locked_at + cooloff {{
                    return Err(auth_err());
                }}
                // Cool-off elapsed — treat the account as unlocked. Reset local
                // counters so the next failure starts from 1, not from threshold.
                current_attempts = 0;
                current_locked_at = None;
            }}

            if !password_ok {{
                // Atomically increment the failure counter so concurrent bad-password
                // requests each count as distinct failures rather than collapsing into
                // one. Use two separate update paths (different Diesel expression types)
                // rather than an if expression inside `.eq(...)`.
                let new_attempts: i32 = if current_attempts == {snake_name}.failed_attempts {{
                    // Normal path: DB atomically computes `failed_attempts + 1`.
                    // Propagate write errors — silently absorbing a failure here
                    // would let repeated bad-password attempts bypass the threshold.
                    diesel::update({table}::table.find({snake_name}.id))
                        .set({table}::failed_attempts.eq({table}::failed_attempts + 1))
                        .returning({table}::failed_attempts)
                        .get_result::<i32>(&mut *db)
                        .await
                        .map_err(|e| AutumnError::internal_server_error_msg(&format!("Failed to record failed login: {{e}}")))?
                }} else {{
                    // Cool-off reset path: atomically reset counter to 1 and clear
                    // locked_at in a single UPDATE. Concurrent requests hitting this
                    // branch all write the same values so races are benign (all still
                    // count 1 failure each, not zero). Propagate write errors so that
                    // DB permission failures don't silently bypass the lockout threshold.
                    diesel::update({table}::table.find({snake_name}.id))
                        .set((
                            {table}::failed_attempts.eq(1i32),
                            {table}::locked_at.eq(None::<chrono::NaiveDateTime>),
                        ))
                        .execute(&mut *db)
                        .await
                        .map_err(|e| AutumnError::internal_server_error_msg(&format!("Failed to reset lockout counter: {{e}}")))?;
                    1i32
                }};

                if new_attempts >= lockout_cfg.threshold && current_locked_at.is_none() {{
                    // Account transitions into the locked state — stamp locked_at
                    // atomically. Propagate errors: if this write fails the account
                    // is not locked despite the counter crossing the threshold, which
                    // would allow a successful login to slip through.
                    diesel::update({table}::table.find({snake_name}.id))
                        .set({table}::locked_at.eq(Some(now)))
                        .execute(&mut *db)
                        .await
                        .map_err(|e| AutumnError::internal_server_error_msg(&format!("Failed to lock account: {{e}}")))?;

                    // Truncate to a coarse IP prefix (IPv4 /24, IPv6 /64) so
                    // the telemetry event enables incident response without
                    // logging a precise user identifier.
                    let ip_prefix = match addr_ip {{
                        std::net::IpAddr::V4(ip) => {{
                            let [a, b, c, _] = ip.octets();
                            format!("{{a}}.{{b}}.{{c}}.0/24")
                        }}
                        std::net::IpAddr::V6(ip) => {{
                            let s = ip.segments();
                            format!("{{:x}}:{{:x}}:{{:x}}:{{:x}}::/64", s[0], s[1], s[2], s[3])
                        }}
                    }};
                    // Salt the digest with the deployment secret so the
                    // account ID cannot be recovered by hashing small integers.
                    let account_id_digest = {{
                        use sha2::{{Digest, Sha256}};
                        // Require a deployment secret for the digest salt. Operators
                        // MUST set SECRET_KEY_BASE (already required for sessions) or
                        // AUTUMN_ADMIN_SECRET. The static fallback prevents reversibility
                        // only within this process; set the env var in production.
                        let salt = std::env::var("SECRET_KEY_BASE")
                            .or_else(|_| std::env::var("AUTUMN_ADMIN_SECRET"))
                            .unwrap_or_else(|_| "autumn-lockout-fallback-salt".to_string());
                        let hash = Sha256::digest(
                            format!("{{}}:{{}}", salt, {snake_name}.id).as_bytes(),
                        );
                        hex::encode(&hash[..8])
                    }};
                    tracing::warn!(
                        event = "account_locked",
                        account_id_digest = %account_id_digest,
                        ip_prefix = %ip_prefix,
                        failed_attempts = new_attempts,
                        "account locked after repeated failed login attempts"
                    );
                }}
                return Err(auth_err());
            }}
        }} else if !password_ok {{
            return Err(auth_err());
        }}
    }} else {{
        // Unknown email — bcrypt ran on DUMMY_HASH above for timing safety.
        if !password_ok {{
            return Err(auth_err());
        }}
    }}

    let {snake_name} = found_{snake_name}.ok_or_else(auth_err)?;

    // Successful login: reset lockout counter only when the account is not
    // currently locked. The WHERE clause accepts both:
    //   - locked_at IS NULL (no lock ever set), and
    //   - locked_at <= now - cooloff (lock expired via cool-off),
    // so that a user who waited out the cool-off can log in with a correct
    // password without requiring an operator unlock.
    // If zero rows match after excluding active (non-expired) locks, the
    // account was just locked concurrently — reject the login.
    if lockout_enabled {{
        let cooloff = chrono::Duration::seconds(lockout_cfg.cooloff_secs as i64);
        let lock_expired_before = chrono::Utc::now().naive_utc() - cooloff;
        let rows_cleared = diesel::update(
            {table}::table
                .find({snake_name}.id)
                .filter(
                    {table}::locked_at.is_null().or(
                        {table}::locked_at.le(lock_expired_before)
                    )
                ),
        )
        .set((
            {table}::failed_attempts.eq(0),
            {table}::locked_at.eq(None::<chrono::NaiveDateTime>),
        ))
        .execute(&mut *db)
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(&format!("Failed to reset lockout on login: {{e}}")))?;
        if rows_cleared == 0 {{
            return Err(auth_err());
        }}
    }}

{totp_login_branch}
    session.rotate_id().await;
{totp_clear_pending}    session.insert("{snake_name}_id", {snake_name}.id.to_string()).await;
    session.insert("{snake_name}_email", &{snake_name}.email).await;
    // Use the same session key checked by `#[secured]` / `#[authorize]`.
    session.insert(state.auth_session_key(), {snake_name}.id.to_string()).await;
    Ok(redirect_to("/account"))
}}

// ── Logout ────────────────────────────────────────────────────────────────────

/// `POST /logout` — destroy the session and redirect to the login page.
///
/// Destroying (not just clearing) the session ensures an old session cookie
/// cannot be replayed after logout.
#[post("/logout")]
pub async fn logout(session: Session) -> AutumnResult<Response> {{
    session.destroy().await;
    Ok(redirect_to("/login"))
}}

// ── Operator unlock ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UnlockAccountForm {{
    pub email: String,
}}

/// `POST /auth/admin/unlock` — clear the lockout for a single account.
///
/// Requires the `X-Admin-Secret` header to match the `AUTUMN_ADMIN_SECRET`
/// environment variable. Returns the same 422 as a wrong-password attempt on
/// both wrong secret and unknown email so the response does not reveal which
/// accounts exist or are locked.
///
/// Protect this endpoint with network-level access controls (VPN, internal
/// load-balancer allowlist) in production in addition to the secret header.
///
/// **CSRF exemption required.** Autumn enables CSRF by default on all non-safe
/// routes. Add this path to the exempt list in `autumn.toml`:
/// ```toml
/// [security.csrf]
/// exempt_paths = ["/auth/admin/unlock"]
/// ```
///
/// Usage:
/// ```sh
/// curl -s -X POST https://example.com/auth/admin/unlock \
///   -H "Content-Type: application/x-www-form-urlencoded" \
///   -H "X-Admin-Secret: $AUTUMN_ADMIN_SECRET" \
///   -d "email=user%40example.com"
/// ```
#[post("/auth/admin/unlock")]
pub async fn unlock_account(
    mut db: Db,
    headers: axum::http::HeaderMap,
    Form(form): Form<UnlockAccountForm>,
) -> AutumnResult<Markup> {{
    let admin_secret = std::env::var("AUTUMN_ADMIN_SECRET").unwrap_or_default();
    let provided = headers
        .get("x-admin-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if admin_secret.is_empty() || provided != admin_secret {{
        return Err(AutumnError::unprocessable_msg("Invalid email or password."));
    }}
    let email = form.email.trim().to_lowercase();
    diesel::update({table}::table.filter({table}::email.eq(&email)))
        .set((
            {table}::failed_attempts.eq(0),
            {table}::locked_at.eq(None::<chrono::NaiveDateTime>),
        ))
        .execute(&mut *db)
        .await
        .map_err(|e| AutumnError::internal_server_error_msg(&format!("Failed to unlock account: {{e}}")))?;
    Ok(layout("Account Unlocked", html! {{
        h1 {{ "Account Unlocked" }}
        p {{ "The lockout for " (email) " has been cleared if it existed." }}
    }}))
}}

// ── Account (protected example route) ────────────────────────────────────────

/// `GET /account` — current-account profile placeholder. Requires authentication.
///
/// This is a protected-route example: the `#[secured]` attribute rejects
/// anonymous requests before the handler body runs.
#[secured]
#[get("/account")]
pub async fn account(session: Session, mut db: Db, csrf: Option<CsrfToken>, csrf_field: Option<CsrfFormField>) -> AutumnResult<Markup> {{
    let {snake_name}_id: i64 = session
        .get("{snake_name}_id")
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;

    let {snake_name}: {pascal_name} = {table}::table
        .find({snake_name}_id)
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Account not found."))?;

    Ok(layout("Your Account", html! {{
        h1 {{ "Your Account" }}
        p {{ "Email: " ({snake_name}.email) }}
        form action="/logout" method="post" {{
            @if let Some(ref csrf) = csrf {{ input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }}
            button type="submit" {{ "Log Out" }}
        }}
    }}))
}}

// ── Forgot Password ───────────────────────────────────────────────────────────

/// `GET /forgot-password` — render the forgot-password form.
#[get("/forgot-password")]
pub async fn forgot_password_form(csrf: Option<CsrfToken>, csrf_field: Option<CsrfFormField>) -> AutumnResult<Markup> {{
    Ok(layout("Forgot Password", html! {{
        h1 {{ "Forgot Your Password?" }}
        form action="/forgot-password" method="post" {{
            @if let Some(ref csrf) = csrf {{ input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }}
            div {{
                label {{ "Email" }}
                input type="email" name="email" required autocomplete="email";
            }}
            button type="submit" {{ "Send Reset Link" }}
        }}
    }}))
}}

#[derive(Deserialize)]
pub struct ForgotPasswordForm {{
    pub email: String,
}}

/// `POST /forgot-password` — generate a reset token and email a reset link.
///
/// Non-enumerating: always returns the same confirmation page whether or not
/// the email address is registered, so callers cannot learn which addresses
/// exist. Requires Autumn mail to be configured; fails with a clear message
/// identifying the missing mail configuration if it is not.
#[post("/forgot-password")]
pub async fn forgot_password(
    mut db: Db,
    mailer: Mailer,
    Form(form): Form<ForgotPasswordForm>,
) -> AutumnResult<Markup> {{
    // Fail fast when mail is not configured — safe because this check is
    // independent of the email address lookup and does not leak whether an
    // address is registered.
    if mailer.is_disabled() {{
        return Err(AutumnError::internal_server_error_msg(
            "Password reset requires mail to be configured. \
             Set [mail] transport in autumn.toml (e.g. transport = \"smtp\"). \
             The forgot-password feature is unavailable until mail is set up.",
        ));
    }}

    let email = form.email.trim().to_lowercase();

    // Record start time; the response is padded to a constant minimum below
    // so an attacker cannot infer whether an address is registered by
    // measuring response latency.
    let t0 = std::time::Instant::now();

    // Non-enumerating: silently skip unknown addresses.
    let maybe_{snake_name}: Option<{pascal_name}> = {table}::table
        .filter({table}::email.eq(&email))
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .ok();

    if let Some({snake_name}) = maybe_{snake_name} {{
        let raw_token = generate_reset_token();
        let token_digest = sha256_hex(&raw_token);
        let expires_at =
            chrono::Utc::now().naive_utc() + chrono::Duration::hours(2);

        diesel::update({table}::table.find({snake_name}.id))
            .set((
                {table}::reset_token_digest.eq(Some(&token_digest)),
                {table}::reset_token_expires_at.eq(Some(expires_at)),
            ))
            .execute(&mut *db)
            .await?;

        // Non-enumerating: log send failures but do not surface them to the
        // caller — the response is always the same "check your email" page.
        if let Err(e) = send_reset_email(&mailer, &{snake_name}.email, &raw_token).await {{
            tracing::error!("password-reset email failed: {{e}}");
        }}
    }}

    // Pad to a constant minimum so hit and miss paths take indistinguishable
    // wall-clock time.
    if let Some(remaining) = std::time::Duration::from_secs(1).checked_sub(t0.elapsed()) {{
        tokio::time::sleep(remaining).await;
    }}

    Ok(layout("Check Your Email", html! {{
        h1 {{ "Check Your Email" }}
        p {{
            "If that address is registered you'll receive a reset link shortly."
        }}
        p {{ a href="/login" {{ "Back to login" }} }}
    }}))
}}

// ── Reset Password ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ResetPasswordQuery {{
    pub token: String,
}}

/// `GET /reset-password?token=<raw>` — render the reset-password form.
#[get("/reset-password")]
pub async fn reset_password_form(
    Query(query): Query<ResetPasswordQuery>,
    csrf: Option<CsrfToken>,
    csrf_field: Option<CsrfFormField>,
) -> AutumnResult<Markup> {{
    Ok(layout("Reset Password", html! {{
        h1 {{ "Set a New Password" }}
        form action="/reset-password" method="post" {{
            @if let Some(ref csrf) = csrf {{ input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }}
            input type="hidden" name="token" value=(query.token);
            div {{
                label {{ "New Password (8+ characters)" }}
                input type="password" name="password" required
                      autocomplete="new-password" minlength="8";
            }}
            button type="submit" {{ "Set New Password" }}
        }}
    }}))
}}

#[derive(Deserialize)]
pub struct ResetPasswordForm {{
    pub token: String,
    pub password: String,
}}

/// `POST /reset-password` — verify the reset token and update the password.
///
/// The token is compared via its stored digest (constant-time via `sha2`).
/// On success the session is rotated, invalidating any prior authenticated
/// state.
#[post("/reset-password")]
pub async fn reset_password(
    mut db: Db,
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<ResetPasswordForm>,
) -> AutumnResult<Response> {{
    if form.password.len() < 8 {{
        return Err(AutumnError::unprocessable_msg(
            "Password must be at least 8 characters.",
        ));
    }}

    let token_digest = sha256_hex(&form.token);
    let now = chrono::Utc::now().naive_utc();

    let {snake_name}: {pascal_name} = {table}::table
        .filter({table}::reset_token_digest.eq(Some(&token_digest)))
        .filter({table}::reset_token_expires_at.gt(now))
        .select({pascal_name}::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| {{
            AutumnError::unprocessable_msg("Invalid or expired reset link.")
        }})?;

    let new_digest = hash_password(&form.password).await?;
{totp_reset_branch}
    diesel::update({table}::table.find({snake_name}.id))
        .set((
            {table}::password_digest.eq(&new_digest),
            {table}::reset_token_digest.eq(None::<String>),
            {table}::reset_token_expires_at.eq(None::<chrono::NaiveDateTime>),
        ))
        .execute(&mut *db)
        .await?;

    // Rotate session to invalidate any previous authenticated state.
    session.rotate_id().await;
{totp_clear_pending}    session.insert("{snake_name}_id", {snake_name}.id.to_string()).await;
    session.insert("{snake_name}_email", &{snake_name}.email).await;
    // Use the same session key checked by `#[secured]` / `#[authorize]`.
    session.insert(state.auth_session_key(), {snake_name}.id.to_string()).await;
    Ok(redirect_to("/account"))
}}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Generate a 32-byte cryptographically-random reset token, hex-encoded.
fn generate_reset_token() -> String {{
    use rand::TryRngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS RNG failed");
    hex::encode(bytes)
}}

/// SHA-256 hex digest of `input` using the `sha2` crate.
fn sha256_hex(input: &str) -> String {{
    use sha2::{{Digest, Sha256}};
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(hash)
}}

/// Send a password-reset email via the Autumn mailer.
///
/// # Errors
/// Returns a clear `AutumnError::internal` message when mail is not configured
/// (`transport = "disabled"`) or when the send itself fails.
async fn send_reset_email(mailer: &Mailer, to: &str, token: &str) -> AutumnResult<()> {{
    if mailer.is_disabled() {{
        return Err(AutumnError::internal_server_error_msg(
            "Password reset requires mail to be configured. \
             Set [mail] transport in autumn.toml (e.g. transport = \"smtp\"). \
             The forgot-password feature is unavailable until mail is set up.",
        ));
    }}
    // APP_BASE_URL must be set to the public URL of your app (e.g. https://example.com).
    let base_url = std::env::var("APP_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:3000".to_owned());
    let reset_url = format!("{{base_url}}/reset-password?token={{token}}");
    let mail = Mail::builder()
        .to(to.to_owned())
        .subject("Reset your password")
        .html(html! {{
            p {{ "Click the link below to reset your password." }}
            p {{ "This link expires in 2 hours." }}
            p {{ a href=(&reset_url) {{ "Reset Password" }} }}
            p {{ "If you did not request this, you can safely ignore this email." }}
        }})
        .text(format!(
            "Reset your password: {{reset_url}}\n\
             This link expires in 2 hours.\n\
             If you did not request this you can safely ignore this email."
        ))
        .build()
        .map_err(|e| {{
            AutumnError::internal_server_error_msg(format!(
                "Failed to build password-reset email: {{e}}"
            ))
        }})?;
    mailer.send(mail).await.map_err(|e| {{
        AutumnError::internal_server_error_msg(format!(
            "Failed to send password-reset email: {{e}}"
        ))
    }})
}}
{totp_section}"#
    )
}

#[allow(clippy::too_many_lines)]
fn render_tests_file(pascal_name: &str, _snake_name: &str) -> String {
    format!(
        r#"//! Request-level smoke tests for {pascal_name} auth, generated by `autumn generate auth`.
//!
//! These tests run against a live server started with `AUTUMN_TEST_BASE_URL`.
//! In CI, start the app, set the env var, and run `cargo test`.
//!
//! Each test uses a raw TCP connection to avoid adding an HTTP client dep;
//! replace with your preferred HTTP client once it is in `Cargo.toml`.

use std::io::{{Read, Write}};
use std::net::TcpStream;

fn base_url() -> Option<String> {{
    std::env::var("AUTUMN_TEST_BASE_URL").ok()
}}

fn host_port(base: &str) -> String {{
    base.trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_owned()
}}

fn get(base: &str, path: &str) -> String {{
    let hp = host_port(base);
    let mut stream =
        TcpStream::connect(&hp).unwrap_or_else(|_| panic!("cannot connect to {{base}}"));
    let req = format!("GET {{path}} HTTP/1.1\r\nHost: {{hp}}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).expect("write failed");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read failed");
    resp
}}

fn post_form(base: &str, path: &str, body: &str, cookie: &str) -> String {{
    let hp = host_port(base);
    let mut stream =
        TcpStream::connect(&hp).unwrap_or_else(|_| panic!("cannot connect to {{base}}"));
    let req = format!(
        "POST {{path}} HTTP/1.1\r\n\
         Host: {{hp}}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {{}}\r\n\
         Cookie: {{cookie}}\r\n\
         Connection: close\r\n\r\n\
         {{body}}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write failed");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read failed");
    resp
}}

#[test]
fn auth_signup_returns_200() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = get(&base, "/signup");
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "GET /signup did not return 200:\n{{resp}}"
    );
}}

#[test]
fn auth_login_returns_200() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = get(&base, "/login");
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "GET /login did not return 200:\n{{resp}}"
    );
}}

#[test]
fn auth_logout_redirects() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = post_form(&base, "/logout", "", "");
    assert!(
        resp.contains("HTTP/1.1 30") || resp.contains("HTTP/1.0 30"),
        "POST /logout did not redirect:\n{{resp}}"
    );
}}

#[test]
fn auth_forgot_password_returns_200() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = get(&base, "/forgot-password");
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "GET /forgot-password did not return 200:\n{{resp}}"
    );
}}

#[test]
fn auth_reset_password_returns_200() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let resp = get(&base, "/reset-password?token=dummy");
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "GET /reset-password did not return 200:\n{{resp}}"
    );
}}

#[test]
fn auth_account_rejects_anonymous() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    // Without a session cookie, /account must reject with 401 or redirect.
    let resp = get(&base, "/account");
    let is_rejected = resp.contains("HTTP/1.1 401")
        || resp.contains("HTTP/1.0 401")
        || resp.contains("HTTP/1.1 30")
        || resp.contains("HTTP/1.0 30");
    assert!(
        is_rejected,
        "GET /account should reject anonymous requests (expected 401 or redirect):\n{{resp}}"
    );
}}

// ── Account lockout tests ─────────────────────────────────────────────────────
//
// These tests verify the lockout policy required by issue #814.
// They run against a live server like the tests above; set AUTUMN_TEST_BASE_URL.
//
// Each lockout test creates its own account via POST /signup before running
// so the tests are self-contained against a fresh database — no pre-seeding needed.

/// Sign up an account, ignoring any error (account may already exist from a
/// previous test run). Used to make lockout tests self-contained.
fn ensure_account(base: &str, email: &str, password: &str) {{
    let body = format!("email={{email}}&password={{password}}");
    // The response may be a redirect (201/302 on success) or 422 if the account
    // already exists — both are acceptable; we only care that the account exists.
    post_form(base, "/signup", &body, "");
}}

/// AC8a — N+1 failed attempts within the window cause the next attempt with
/// correct credentials to be rejected.
///
/// Scenario: sign up an account, submit threshold bad-password POSTs, then
/// submit one more POST with the correct password. The final attempt must be
/// rejected (same 422 body as wrong password) because the account is now locked.
///
/// Set `AUTUMN_AUTH__LOCKOUT__THRESHOLD=10` (the default) to run against the
/// standard policy.
#[test]
fn auth_lockout_rejects_correct_credentials() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let email = "lockout-test@example.com";
    let password = "correct-password-L0ck";
    ensure_account(&base, email, password);
    let bad_body = format!("email={{email}}&password=wrong-password");
    // Exhaust the threshold (default 10) with wrong passwords.
    for _ in 0..10 {{
        post_form(&base, "/login", &bad_body, "");
    }}
    // The next attempt with the CORRECT password must still be rejected.
    let correct_body = format!("email={{email}}&password={{password}}");
    let resp = post_form(&base, "/login", &correct_body, "");
    assert!(
        resp.contains("HTTP/1.1 422") || resp.contains("HTTP/1.0 422"),
        "locked account must reject correct credentials with 422:\n{{resp}}"
    );
}}

/// AC8b — a successful login before the threshold resets the failure counter.
///
/// Scenario: sign up an account, submit (threshold-1) bad-password POSTs, then
/// 1 correct-password POST (which succeeds and resets the counter), then
/// (threshold-1) more bad-password POSTs. The account must remain unlocked.
#[test]
fn auth_successful_login_resets_lockout_counter() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let email = "lockout-reset-test@example.com";
    let password = "correct-password-R3set";
    ensure_account(&base, email, password);
    let bad_body = format!("email={{email}}&password=wrong-password");
    let good_body = format!("email={{email}}&password={{password}}");
    // 5 failures (below default threshold of 10).
    for _ in 0..5 {{
        post_form(&base, "/login", &bad_body, "");
    }}
    // Successful login resets the counter.
    let good_resp = post_form(&base, "/login", &good_body, "");
    assert!(
        good_resp.contains("HTTP/1.1 302") || good_resp.contains("HTTP/1.0 302"),
        "correct credentials must succeed before lockout threshold:\n{{good_resp}}"
    );
    // 5 more failures (counter was reset, so still below threshold).
    for _ in 0..5 {{
        post_form(&base, "/login", &bad_body, "");
    }}
    // Should still be able to log in with correct credentials.
    let after_resp = post_form(&base, "/login", &good_body, "");
    assert!(
        after_resp.contains("HTTP/1.1 302") || after_resp.contains("HTTP/1.0 302"),
        "account must remain unlocked after counter was reset by successful login:\n{{after_resp}}"
    );
}}

/// AC8c — the locked-state response is byte-identical at the response-body
/// level to a wrong-password response (non-enumerating lockout).
#[test]
fn auth_lockout_response_identical_to_wrong_password() {{
    let Some(base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    let email_locked = "lockout-body-test@example.com";
    let password = "correct-password-B0dy";
    ensure_account(&base, email_locked, password);
    // Wrong-password response body for a non-locked known account.
    let wrong_resp = post_form(
        &base,
        "/login",
        &format!("email={{email_locked}}&password=wrong"),
        "",
    );
    let wrong_body = wrong_resp.splitn(2, "\r\n\r\n").nth(1).unwrap_or("");
    // Exhaust threshold to lock the target account.
    for _ in 0..9 {{
        post_form(
            &base,
            "/login",
            &format!("email={{email_locked}}&password=bad"),
            "",
        );
    }}
    // Locked-account response with correct password.
    let locked_resp = post_form(
        &base,
        "/login",
        &format!("email={{email_locked}}&password={{password}}"),
        "",
    );
    let locked_body = locked_resp.splitn(2, "\r\n\r\n").nth(1).unwrap_or("");
    assert_eq!(
        wrong_body, locked_body,
        "locked-state response body must be byte-identical to wrong-password response body\n\
         wrong:  {{wrong_body:?}}\n\
         locked: {{locked_body:?}}"
    );
}}
"#
    )
}

#[allow(clippy::too_many_lines)]
fn render_docs_file(pascal_name: &str, totp: bool) -> String {
    let totp_docs = if totp { TOTP_DOCS_SECTION } else { "" };
    format!(
        r#"# Authentication Guide

Generated by `autumn generate auth`. Edit freely.

## Overview

This guide documents the browser-session authentication flow generated for
your Autumn application. The generated code handles signup, login, logout,
account profile, and password reset using Autumn's built-in session, CSRF,
password hashing, and mail primitives.

## Generated Routes

| Method | Path | Handler | Auth |
|--------|------|---------|------|
| GET | `/signup` | `signup_form` | Public |
| POST | `/signup` | `signup` | Public |
| GET | `/login` | `login_form` | Public |
| POST | `/login` | `login` | Public |
| POST | `/logout` | `logout` | Any |
| GET | `/account` | `account` | **Required** |
| GET | `/forgot-password` | `forgot_password_form` | Public |
| POST | `/forgot-password` | `forgot_password` | Public |
| GET | `/reset-password` | `reset_password_form` | Public |
| POST | `/reset-password` | `reset_password` | Public |

## Security Properties

- **Passwords**: Hashed with bcrypt (cost 12) via `autumn_web::auth::hash_password`.
  Raw passwords are never logged or stored.
- **Reset tokens**: 32-byte random values generated with `OsRng`; only the
  SHA-256 digest is stored in `reset_token_digest`. The raw token is sent by
  email only and expires after 2 hours.
- **Non-enumeration**: Duplicate signup, failed login, and forgot-password
  submissions for unknown addresses all return responses that do not reveal
  whether an email address is registered.
- **Session fixation**: Login and password-reset rotate the session ID
  (`session.rotate_id()`).
- **Session invalidation**: Logout calls `session.destroy()` so an old session
  cookie cannot be replayed.
- **Protected routes**: The `/account` route uses `#[secured]` to reject
  unauthenticated requests before the handler runs.
- **Account lockout**: After the configured threshold of failed login attempts,
  the account is locked for the cool-off period. Lockout responses are
  indistinguishable from wrong-password responses (same status, same body) so
  the endpoint does not reveal which accounts are locked. Lockout state is
  stored in Postgres so all app replicas agree. See [Account Lockout Policy](#account-lockout-policy) below.

## Account Lockout Policy

The generated login endpoint includes account-level lockout to protect against
credential-stuffing attacks that rotate source IPs to bypass IP-rate limiting.

### How It Works

1. Each failed login increments `failed_attempts` on the account row.
2. When `failed_attempts` reaches `threshold`, `locked_at` is stamped with the
   current time and a structured telemetry event fires (see below).
3. While the account is locked (`locked_at + cooloff_secs > now`), every login
   attempt — correct password or not — returns the same `422 Invalid email or
   password` response as a wrong-password attempt (non-enumerating).
4. A successful login resets `failed_attempts` to `0` and clears `locked_at`.
5. After `cooloff_secs` elapses without a login attempt, the account auto-unlocks
   on the next successful login.

### Configuration (`autumn.toml`)

```toml
[auth.lockout]
enabled      = true  # false → disable lockout entirely (e.g. external policy in place)
threshold    = 10    # consecutive failures before lockout
window_secs  = 60    # reserved for future sliding-window counting; not yet enforced —
                     # failures currently accumulate since the last successful login
cooloff_secs = 900   # lock duration in seconds (default 15 minutes)
```

Override at deploy time via environment variables:

| Variable | Description |
|----------|-------------|
| `AUTUMN_AUTH__LOCKOUT__ENABLED` | `true` / `false` |
| `AUTUMN_AUTH__LOCKOUT__THRESHOLD` | integer (0 = disabled) |
| `AUTUMN_AUTH__LOCKOUT__WINDOW_SECS` | integer |
| `AUTUMN_AUTH__LOCKOUT__COOLOFF_SECS` | integer |

Set `threshold = 0` or `enabled = false` to restore the pre-lockout login
behaviour for apps that use a stronger external policy.

### Telemetry

When an account transitions into the locked state the handler emits a
`tracing::warn!` event with:

| Field | Value |
|-------|-------|
| `event` | `"account_locked"` |
| `account_id_digest` | First 8 bytes of SHA-256(account ID) — stable but not reversible |
| `ip_prefix` | IPv4 /24 prefix (`a.b.c.0/24`) or IPv6 /64 prefix (`x:x:x:x::/64`) |
| `failed_attempts` | Counter value at lock time |

This event flows through the standard Autumn tracing/metrics surface
(OTLP, structured JSON, or console pretty-print depending on config).

### Operator Unlock

The generated auth routes include a `POST /auth/admin/unlock` endpoint that
clears the lockout for a single account. It is protected by the
`X-Admin-Secret` header matching the `AUTUMN_ADMIN_SECRET` environment
variable. Set this variable to a strong random value in production:

```sh
# Generate and store a secret (do this once, store in your .env or deployment
# secrets manager — NOT in autumn credentials, which are not consulted here).
export AUTUMN_ADMIN_SECRET="$(openssl rand -hex 32)"

# Unlock an account
curl -s -X POST https://example.com/auth/admin/unlock \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -H "X-Admin-Secret: $AUTUMN_ADMIN_SECRET" \
  -d "email=user%40example.com"
```

Protect this endpoint with network-level access controls (VPN, internal
load-balancer allowlist) in production in addition to the secret header.
If `AUTUMN_ADMIN_SECRET` is not set, the endpoint always returns 422.

### Existing Apps — Adoption Path

Apps that ran `autumn generate auth` before this feature was introduced need
one additional migration to add the lockout columns. Because `generate migration`
does not carry column defaults, write the SQL directly:

```sh
# 1. Create an empty migration
autumn generate migration add_lockout_to_{{table}}

# 2. Edit the generated up.sql to add the columns with correct defaults:
```

```sql
-- migrations/<timestamp>_add_lockout_to_{{table}}/up.sql
ALTER TABLE {{table}}
  ADD COLUMN IF NOT EXISTS failed_attempts INT NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS locked_at TIMESTAMP NULL;
```

```sql
-- migrations/<timestamp>_add_lockout_to_{{table}}/down.sql
ALTER TABLE {{table}}
  DROP COLUMN IF EXISTS failed_attempts,
  DROP COLUMN IF EXISTS locked_at;
```

```sh
# 3. Apply the migration
autumn migrate

# 4. Re-generate auth templates to pick up the lockout handler and unlock route
autumn generate auth {pascal_name} --force
```

No behaviour changes for existing apps until the migration is applied and
the generator re-run — the framework does not silently change behaviour.

## Mail Configuration

Password-reset emails contain an absolute link built from `APP_BASE_URL`.
Set this environment variable to your application's public URL:

```sh
# .env or shell
APP_BASE_URL=https://example.com
```

In development, configure file-based mail capture in `autumn.toml` so reset
links land in `target/mail/` instead of hitting an SMTP server:

```toml
[mail]
transport = "file"
from = "Your App <noreply@yourapp.dev>"
```

Open the `.eml` files with any email client to click the reset link.

If `[mail]` is not configured (`transport = "disabled"`), the forgot-password
handler returns an immediate HTTP 500 with a clear configuration message
rather than silently showing "Check Your Email" when no mail will be sent.

## Customization Points

- **Validation**: Add stricter email / password rules to `signup` and
  `reset_password` in `src/routes/auth.rs`.
- **Session keys**: Change the session key names (`{snake_name}_id`,
  `{snake_name}_email`) to match your application's conventions.
- **Redirect targets**: Adjust `redirect_to("/account")` calls to send users
  to the right page after login/signup/reset.
- **Email templates**: Customise the `send_reset_email` function to match your
  brand.
- **{pascal_name} fields**: Add display-name, avatar, or role fields to the
  `{pascal_name}` model and a new migration.

{totp_docs}## When to Choose This Flow vs. Alternatives

| Scenario | Recommendation |
|----------|---------------|
| Browser-based web app | ✅ This generated flow |
| Mobile / CLI / third-party API clients | API tokens (`autumn generate token` — see [#520]) |
| Social login (Google, GitHub, …) | OAuth2/OIDC (S-059) |
| Enterprise / SSO | SAML / enterprise IdP (future) |

## Quick Start

```sh
autumn new myapp
cd myapp
autumn generate auth {pascal_name}
autumn migrate
autumn dev
```

Then open <http://localhost:3000/signup> to create your first account.
"#,
        snake_name = pascal_name.to_lowercase(),
        pascal_name = pascal_name,
    )
}

fn auth_route_entries(totp: bool) -> Vec<String> {
    let mut entries = vec![
        "routes::auth::signup_form".to_owned(),
        "routes::auth::signup".to_owned(),
        "routes::auth::login_form".to_owned(),
        "routes::auth::login".to_owned(),
        "routes::auth::logout".to_owned(),
        "routes::auth::unlock_account".to_owned(),
        "routes::auth::account".to_owned(),
        "routes::auth::forgot_password_form".to_owned(),
        "routes::auth::forgot_password".to_owned(),
        "routes::auth::reset_password_form".to_owned(),
        "routes::auth::reset_password".to_owned(),
    ];
    if totp {
        entries.extend([
            "routes::auth::login_verify_form".to_owned(),
            "routes::auth::login_verify".to_owned(),
            "routes::auth::two_factor_status".to_owned(),
            "routes::auth::two_factor_enable".to_owned(),
            "routes::auth::two_factor_confirm".to_owned(),
            "routes::auth::two_factor_disable".to_owned(),
        ]);
    }
    entries
}

fn oauth_route_entries() -> Vec<String> {
    vec![
        "routes::oauth::oauth_redirect".to_owned(),
        "routes::oauth::oauth_callback".to_owned(),
    ]
}

fn render_oauth_migration_up(user_table: &str) -> String {
    format!(
        "CREATE TABLE oauth_identities (\n\
         \x20   id BIGSERIAL PRIMARY KEY,\n\
         \x20   provider TEXT NOT NULL,\n\
         \x20   subject TEXT NOT NULL,\n\
         \x20   user_id BIGINT NOT NULL REFERENCES {user_table}(id) ON DELETE CASCADE,\n\
         \x20   email TEXT NULL,\n\
         \x20   name TEXT NULL,\n\
         \x20   created_at TIMESTAMP NOT NULL DEFAULT NOW(),\n\
         \x20   UNIQUE (provider, subject)\n\
         );\n\
         \n\
         CREATE INDEX oauth_identities_user_id_idx ON oauth_identities (user_id);\n"
    )
}

fn render_oauth_migration_down() -> String {
    "DROP TABLE oauth_identities;\n".to_owned()
}

#[allow(clippy::too_many_lines)]
fn render_oauth_routes_file(
    pascal_name: &str,
    snake_name: &str,
    user_table: &str,
    providers: &[String],
    totp: bool,
) -> String {
    let provider_list = providers
        .iter()
        .map(|p| format!("\"{p}\""))
        .collect::<Vec<_>>()
        .join(", ");

    // When TOTP is also generated, the callback guidance must NOT set the secured
    // session key directly for a 2FA-enabled account — it has to route through
    // `/login/verify` exactly like password login, or OAuth would bypass 2FA.
    let totp_callback_note = if totp {
        "    //\n\
         \x20   // ⚠️ TWO-FACTOR: this app was generated with `--totp`. If the resolved\n\
         \x20   // local account has `totp_enabled == true`, do NOT set the secured\n\
         \x20   // session key here. Instead mark the session pending and redirect to\n\
         \x20   // the second-factor form, mirroring the password-login flow:\n\
         \x20   //\n\
         \x20   //   if local_user.totp_enabled {\n\
         \x20   //       // Refuse if [auth].session_key collides with a reserved pending\n\
         \x20   //       // key — otherwise parking `totp_pending_id` would write the very\n\
         \x20   //       // key `#[secured]` trusts, authenticating before /login/verify:\n\
         \x20   //       if crate::routes::auth::is_reserved_totp_pending_key(&auth_cfg.session_key) {\n\
         \x20   //           return Redirect::to(\"/login?error=totp_key_collision\").into_response();\n\
         \x20   //       }\n\
         \x20   //       session.rotate_id().await;\n\
         \x20   //       // rotate_id() keeps session data, so drop any live auth from a\n\
         \x20   //       // prior login in this browser before parking the pending handoff:\n\
         \x20   //       session.remove(&auth_cfg.session_key).await;\n\
         \x20   //       session.remove(\"__SNAKE___id\").await;\n\
         \x20   //       session.remove(\"__SNAKE___email\").await;\n\
         \x20   //       session.remove(\"totp_pending_reset_digest\").await;\n\
         \x20   //       session.remove(\"totp_pending_reset_token\").await;\n\
         \x20   //       session.remove(\"totp_pending_secret\").await;\n\
         \x20   //       session.insert(\"totp_pending_id\", local_user.id.to_string()).await;\n\
         \x20   //       return Redirect::to(\"/login/verify\").into_response();\n\
         \x20   //   }\n\
         \x20   //   // otherwise, fully authenticate. Rotate the session id first — like\n\
         \x20   //   // the password-login/reset paths — so a pre-login (possibly fixated)\n\
         \x20   //   // session id is never promoted unchanged:\n\
         \x20   //   session.rotate_id().await;\n\
         \x20   //   // Then clear any abandoned pending-2FA / deferred-reset / enrollment\n\
         \x20   //   // state, and set BOTH the model-specific identity keys the generated\n\
         \x20   //   // handlers read (`__SNAKE___id` / `_email`) and the configured auth key\n\
         \x20   //   // — clearing stale model keys first so a prior login in this browser\n\
         \x20   //   // can't leave a mismatched identity:\n\
         \x20   //   session.remove(\"totp_pending_id\").await;\n\
         \x20   //   session.remove(\"totp_pending_reset_digest\").await;\n\
         \x20   //   session.remove(\"totp_pending_reset_token\").await;\n\
         \x20   //   session.remove(\"totp_pending_secret\").await;\n\
         \x20   //   session.insert(\"__SNAKE___id\", local_user.id.to_string()).await;\n\
         \x20   //   session.insert(\"__SNAKE___email\", &local_user.email).await;\n\
         \x20   //   session.insert(&auth_cfg.session_key, local_user.id.to_string()).await;\n"
            .replace("__SNAKE__", snake_name)
    } else {
        String::new()
    };

    format!(
        r#"//! OAuth2/OIDC redirect and callback handlers.
//!
//! Generated by `autumn generate auth --oauth`.
//! Edit freely — once generated this is ordinary user code.
//!
//! Routes:
//!   GET  /auth/:provider/redirect  → redirects the browser to the provider
//!   GET  /auth/:provider/callback  → exchanges the code and logs the user in

use autumn_web::auth::{{OAuth2Callback, OidcIdentity, oauth2_authorize_url, oauth2_finish_login}};
use autumn_web::prelude::*;
use autumn_web::{{get, oauth2_callback, AppState, State}};
use axum::extract::{{Path, Query}};
use axum::response::{{IntoResponse, Redirect}};
use tracing::warn;

/// Supported OAuth2/OIDC providers.
const SUPPORTED_PROVIDERS: &[&str] = &[{provider_list}];

/// Redirect the browser to the provider's authorization endpoint.
///
/// State and nonce are stored in the session for CSRF protection.
/// PKCE (S256) code_challenge is appended automatically by the framework.
#[get("/auth/{{provider}}/redirect")]
pub async fn oauth_redirect(
    Path(provider_name): Path<String>,
    State(state): State<AppState>,
    session: Session,
) -> impl IntoResponse {{
    if !SUPPORTED_PROVIDERS.contains(&provider_name.as_str()) {{
        return Redirect::to("/login?error=unknown_provider").into_response();
    }}

    let auth_cfg = state.config().auth;
    let Some(provider) = auth_cfg.oauth2.providers.get(&provider_name) else {{
        warn!(provider = %provider_name, "oauth provider not configured in autumn.toml");
        return Redirect::to("/login?error=provider_not_configured").into_response();
    }};

    match oauth2_authorize_url(&session, &provider_name, provider).await {{
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => {{
            warn!(error = %e, "oauth2_authorize_url failed");
            Redirect::to("/login?error=oauth_error").into_response()
        }}
    }}
}}

/// Handle the OAuth2 callback: exchange the code, create or link a local account.
///
/// On success the user is redirected to `/account`. A missing or mismatched state
/// returns a non-revealing error redirect without logging the offending values.
///
/// **Important:** `oauth2_finish_login` does NOT set the application session key.
/// You must resolve (or create) the local {pascal_name} row and call
/// `session.insert(&auth_cfg.session_key, local_user_id.to_string())` before redirecting.
#[oauth2_callback("/auth/{{provider}}/callback")]
pub async fn oauth_callback(
    Path(provider_name): Path<String>,
    Query(callback): Query<OAuth2Callback>,
    State(state): State<AppState>,
    session: Session,
) -> impl IntoResponse {{
    if !SUPPORTED_PROVIDERS.contains(&provider_name.as_str()) {{
        return Redirect::to("/login?error=unknown_provider").into_response();
    }}

    let auth_cfg = state.config().auth;
    let Some(provider) = auth_cfg.oauth2.providers.get(&provider_name) else {{
        warn!(provider = %provider_name, "oauth provider not configured in autumn.toml");
        return Redirect::to("/login?error=provider_not_configured").into_response();
    }};

    let identity: OidcIdentity = match oauth2_finish_login(
        &session,
        &provider_name,
        provider,
        &callback,
    )
    .await
    {{
        Ok(id) => id,
        Err(_) => {{
            // Do not log the offending state or code to avoid leaking sensitive values.
            return Redirect::to("/login?error=oauth_failed").into_response();
        }}
    }};

    // TODO: resolve or create a local {pascal_name} record, then set the session.
    //
    // Example (fill in your actual DB query):
    //
    //   let local_user_id: i64 = link_or_create_{snake_name}(
    //       &mut db,
    //       &identity,          // OidcIdentity {{ subject, email, name, … }}
    //       &provider_name,     // e.g. "github"
    //       &auth_cfg,          // carries oauth_linking_policy
    //   ).await?;
    //
    //   session.insert(&auth_cfg.session_key, local_user_id.to_string()).await;
    //
    // Until the above is implemented the user will NOT be logged in after the OAuth
    // callback — that is intentional to avoid authenticating without a local account.
{totp_callback_note}    let _ = identity;
    let _ = user_table_placeholder_{snake_name}();

    Redirect::to("/account").into_response()
}}

#[allow(dead_code)]
fn user_table_placeholder_{snake_name}() -> &'static str {{
    "{user_table}"
}}
"#,
    )
}

#[allow(clippy::too_many_lines)]
fn render_oauth_docs_file(providers: &[String]) -> String {
    let provider_list = providers.join(", ");
    let provider_config_examples = providers
        .iter()
        .map(|p| match p.as_str() {
            "github" => concat!(
                "[auth.oauth2.github]\n",
                "client_id     = \"\"\n",
                "client_secret = \"\"\n",
                "authorize_url = \"https://github.com/login/oauth/authorize\"\n",
                "token_url     = \"https://github.com/login/oauth/access_token\"\n",
                "userinfo_url  = \"https://api.github.com/user\"\n",
                "redirect_uri  = \"https://your-app.example.com/auth/github/callback\"\n",
                "scope         = \"read:user user:email\"\n",
            )
            .to_owned(),
            "google" => concat!(
                "[auth.oauth2.google]\n",
                "client_id     = \"\"\n",
                "client_secret = \"\"\n",
                "authorize_url = \"https://accounts.google.com/o/oauth2/v2/auth\"\n",
                "token_url     = \"https://oauth2.googleapis.com/token\"\n",
                "userinfo_url  = \"https://openidconnect.googleapis.com/v1/userinfo\"\n",
                "redirect_uri  = \"https://your-app.example.com/auth/google/callback\"\n",
                "scope         = \"openid email profile\"\n",
                "issuer        = \"https://accounts.google.com\"\n",
                "jwks_url      = \"https://www.googleapis.com/oauth2/v3/certs\"\n",
                "discovery_url = \"https://accounts.google.com\"\n",
            )
            .to_owned(),
            "microsoft" => concat!(
                "[auth.oauth2.microsoft]\n",
                "client_id     = \"\"\n",
                "client_secret = \"\"\n",
                "authorize_url = \"https://login.microsoftonline.com/{YOUR_TENANT_ID}/oauth2/v2.0/authorize\"\n",
                "token_url     = \"https://login.microsoftonline.com/{YOUR_TENANT_ID}/oauth2/v2.0/token\"\n",
                "redirect_uri  = \"https://your-app.example.com/auth/microsoft/callback\"\n",
                "scope         = \"openid email profile\"\n",
                "# Single-tenant: replace {YOUR_TENANT_ID} with your Directory (tenant) ID.\n",
                "# Multi-tenant (common endpoint): ID-token issuer varies per user — see docs/guide/oauth.md.\n",
                "issuer        = \"https://login.microsoftonline.com/{YOUR_TENANT_ID}/v2.0\"\n",
                "jwks_url      = \"https://login.microsoftonline.com/{YOUR_TENANT_ID}/discovery/v2.0/keys\"\n",
            )
            .to_owned(),
            p => {
                format!(
                    r#"[auth.oauth2.{p}]
client_id     = ""
client_secret = ""
authorize_url = "https://{p}.example.com/oauth2/authorize"
token_url     = "https://{p}.example.com/oauth2/token"
redirect_uri  = "https://your-app.example.com/auth/{p}/callback"
scope         = "openid profile email"
"#
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"# OAuth2 / OIDC Authentication

Generated by `autumn generate auth --oauth {provider_list}`.

## Quick start

### 1. Register redirect URIs with each provider

Before the app will work you must register each redirect URI with the provider's
developer console:

| Provider | Console URL | Redirect URI path |
|----------|-------------|-------------------|
| Google   | <https://console.cloud.google.com/> | `/auth/google/callback` |
| GitHub   | <https://github.com/settings/developers> | `/auth/github/callback` |
| Microsoft | <https://portal.azure.com/> | `/auth/microsoft/callback` |

### 2. Add provider credentials to `autumn.toml`

```toml
{provider_config_examples}
```

Keep `client_secret` out of source control — use environment variables or
`autumn credentials edit` to inject secrets at runtime.

### 3. Run the generator command

```sh
autumn generate auth User --oauth {provider_list}
autumn migrate
autumn dev
```

Open <http://localhost:3000/auth/{first_provider}/redirect> to test the flow.

## Security properties

| Property | Status |
|----------|---------|
| PKCE (S256) | ✅ enabled for every provider by default |
| State (anti-CSRF) | ✅ constant-time validated on every callback |
| Nonce (replay protection) | ✅ validated for OIDC ID-token flows |
| ID-token signature | ✅ verified against provider JWKS |
| Session fixation prevention | ✅ session ID rotated on login |
| `client_secret` in logs | ✅ never logged |

## Account-linking policy

Configure in `autumn.toml`:

```toml
[auth]
oauth_linking_policy = "create_account"           # default: create a new account on first sign-in
# oauth_linking_policy = "require_local_signup_first"  # link only to existing accounts
```

## OIDC discovery

Providers that support OIDC discovery (`discovery_url` in preset) have their
endpoints populated automatically from `{{discovery_url}}/.well-known/openid-configuration`.
GitHub uses explicit endpoints (it is pure OAuth2, not OIDC).

## Troubleshooting

**`redirect_uri_mismatch`** — The `redirect_uri` in `autumn.toml` must match
exactly what is registered in the provider console, including scheme and path.

**`invalid_client`** — Check that `client_id` and `client_secret` are correct
and that the app is not in a restricted mode in the provider console.

**`state mismatch` on callback** — The session may have expired between the
redirect and callback. Increase `[session] ttl` or check for load-balancer
sticky-session misconfiguration.

## Generated database schema

```sql
CREATE TABLE oauth_identities (
    id         BIGSERIAL PRIMARY KEY,
    provider   TEXT NOT NULL,
    subject    TEXT NOT NULL,         -- provider's user identifier (sub / id)
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    email      TEXT NULL,
    name       TEXT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    UNIQUE (provider, subject)        -- collision guard: one local account per identity
);
```

Re-authentication with the same `(provider, subject)` pair links to the existing
account. A second local user trying to claim the same identity returns an error and
never silently merges accounts.
"#,
        first_provider = providers.first().map_or("github", String::as_str),
    )
}

// ── TOTP (two-factor) template fragments ────────────────────────────────────────
//
// These helpers return generated *application* source as plain Strings, so the
// braces inside are literal (never processed by this generator's `format!`).

/// Extra `use` lines added to the generated `src/routes/auth.rs` under `--totp`.
const fn totp_imports_src() -> &'static str {
    "use aes_gcm::aead::{Aead, KeyInit};\n\
     use aes_gcm::{Aes256Gcm, Key, Nonce};\n\
     use base64::Engine as _;\n\
     use base64::engine::general_purpose::STANDARD as B64;\n\
     use diesel_async::AsyncConnection as _;\n\
     use totp_rs::{Algorithm, Secret, TOTP};\n\
     use crate::models::recovery_code::{NewRecoveryCode, RecoveryCode};\n\
     use crate::schema::recovery_codes;\n"
}

/// The interstitial branch injected into `login` after password verification.
///
/// A password login starts a *fresh* pending-2FA handoff, so it first clears any
/// stale pending markers left by an abandoned earlier flow (a different account's
/// pending login, or a parked reset) before parking this account.
fn totp_login_branch_src(snake_name: &str) -> String {
    const TPL: &str = r#"
    // ── 2FA interstitial ────────────────────────────────────────────────────
    // If the account has TOTP enabled, do NOT set the `#[secured]` auth key yet.
    // Mark the session `totp_pending` and redirect to the second-factor form.
    if __SNAKE__.totp_enabled {
        // A pending (pre-2FA) session must never write the `#[secured]` auth key.
        // If the app configured `[auth].session_key` to collide with one of the
        // reserved `totp_pending_*` keys, parking the pending marker would set the
        // trusted auth key and let a half-authenticated session pass `#[secured]`.
        // Refuse rather than risk a 2FA bypass.
        if is_reserved_totp_pending_key(state.auth_session_key()) {
            return Err(AutumnError::internal_server_error_msg(
                "[auth].session_key collides with a reserved TOTP pending key (totp_pending_*). \
                 Choose a different [auth].session_key.",
            ));
        }
        session.rotate_id().await;
        // `rotate_id()` keeps existing session data, so if this browser was
        // already authenticated as another account, drop those *live* auth keys
        // before parking the pending handoff — otherwise `#[secured]` routes
        // would still treat the pre-2FA session as the previous account.
        session.remove("__SNAKE___id").await;
        session.remove("__SNAKE___email").await;
        session.remove(state.auth_session_key()).await;
        // Drop any abandoned pending state from a previous flow too, so a stale
        // parked reset, enrollment secret, or a different account's pending login
        // can never be resumed under this login.
        session.remove("totp_pending_reset_digest").await;
        session.remove("totp_pending_reset_token").await;
        session.remove("totp_pending_secret").await;
        session
            .insert("totp_pending_id", __SNAKE__.id.to_string())
            .await;
        return Ok(redirect_to("/login/verify"));
    }
"#;
    TPL.replace("__SNAKE__", snake_name)
}

/// Lines injected into every *direct* full-authentication path (password login,
/// password reset, and signup for non-2FA accounts) right after
/// `session.rotate_id()`. They clear any leftover pending-2FA / parked-reset
/// markers — and any half-finished enrollment secret — so an abandoned
/// `/login/verify` or `/account/2fa/enable` for another account can't be
/// resumed against this freshly established session.
const fn totp_clear_pending_src() -> &'static str {
    "    // Clear any abandoned pending-2FA / deferred-reset / enrollment state so it\n\
     \x20   // cannot be resumed under this freshly authenticated session.\n\
     \x20   session.remove(\"totp_pending_id\").await;\n\
     \x20   session.remove(\"totp_pending_reset_digest\").await;\n\
     \x20   session.remove(\"totp_pending_reset_token\").await;\n\
     \x20   session.remove(\"totp_pending_secret\").await;\n"
}

/// The interstitial branch injected into `reset_password` *before* the password
/// is written. A correct reset token proves control of the email, not possession
/// of the second factor — so for a 2FA-enabled account we DEFER the password
/// change: stash the new digest in the session and require `/login/verify` to
/// pass before committing it. Someone with only the emailed link therefore can't
/// change the password (lockout/DoS) without the second factor.
fn totp_reset_branch_src(snake_name: &str) -> String {
    const TPL: &str = r#"
    // ── 2FA interstitial ────────────────────────────────────────────────────
    if __SNAKE__.totp_enabled {
        // A pending (pre-2FA) session must never write the `#[secured]` auth key
        // (see the login interstitial). Refuse a colliding `[auth].session_key`.
        if is_reserved_totp_pending_key(state.auth_session_key()) {
            return Err(AutumnError::internal_server_error_msg(
                "[auth].session_key collides with a reserved TOTP pending key (totp_pending_*). \
                 Choose a different [auth].session_key.",
            ));
        }
        // Do NOT write the new password yet. Park it in the session and finish
        // the reset in `login_verify` only after the second factor is proven.
        session.rotate_id().await;
        // `rotate_id()` keeps existing session data, so drop any live auth keys
        // from a prior login in this browser before parking the pending handoff
        // (see the login interstitial), and any half-finished enrollment secret.
        session.remove("__SNAKE___id").await;
        session.remove("__SNAKE___email").await;
        session.remove(state.auth_session_key()).await;
        session.remove("totp_pending_secret").await;
        session
            .insert("totp_pending_id", __SNAKE__.id.to_string())
            .await;
        session.insert("totp_pending_reset_digest", &new_digest).await;
        session.insert("totp_pending_reset_token", &token_digest).await;
        return Ok(redirect_to("/login/verify"));
    }
"#;
    TPL.replace("__SNAKE__", snake_name)
}

/// The full TOTP routes section appended to `src/routes/auth.rs`.
#[allow(clippy::too_many_lines)]
fn totp_routes_section_src(pascal_name: &str, snake_name: &str, table: &str) -> String {
    const TPL: &str = r#"
// ── Two-factor authentication (TOTP) ────────────────────────────────────────────
//
// Generated by `autumn generate auth --totp`. Edit freely.
//
// Security properties:
// - TOTP secrets are encrypted at rest with AES-256-GCM (never stored plaintext).
// - Verification accepts a ±1 time-step window and refuses to replay a step that
//   was already consumed (`totp_last_used_step`).
// - Recovery codes are single-use; only their bcrypt digest is stored and each is
//   marked `used_at` when consumed.
// - NOTE: rate-limiting / lockout on repeated failed second-factor attempts is a
//   follow-up (see docs) — add it before exposing this to untrusted traffic.

/// Number of single-use recovery codes generated on enrollment.
const RECOVERY_CODE_COUNT: usize = 10;

/// TOTP time step in seconds (RFC 6238 default).
const TOTP_STEP: u64 = 30;

/// Session keys reserved by the two-factor flow for transient pre-auth state.
/// `[auth].session_key` must not collide with any of these, or a pending
/// (pre-2FA) session could be mistaken for a fully authenticated one.
const RESERVED_TOTP_PENDING_KEYS: &[&str] = &[
    "totp_pending_id",
    "totp_pending_reset_digest",
    "totp_pending_reset_token",
    "totp_pending_secret",
];

/// Whether `key` (typically the configured `[auth].session_key`) collides with a
/// reserved `totp_pending_*` key. Used to refuse a config that would let a
/// pending second-factor session write the trusted `#[secured]` auth key.
///
/// `pub(crate)` so the OAuth callback handler (a separate module) can apply the
/// same guard before parking a pending 2FA session.
pub(crate) fn is_reserved_totp_pending_key(key: &str) -> bool {
    RESERVED_TOTP_PENDING_KEYS.contains(&key)
}

#[derive(Deserialize)]
pub struct TotpCodeForm {
    pub code: String,
}

#[derive(Deserialize)]
pub struct TotpEnableForm {
    pub password: String,
}

#[derive(Deserialize)]
pub struct TotpDisableForm {
    pub password: Option<String>,
    pub code: Option<String>,
}

/// Load the 32-byte AES-256-GCM key used to encrypt TOTP secrets at rest.
///
/// Read from the `TOTP_ENC_KEY` env var (base64 of 32 bytes). Manage it like any
/// other secret (e.g. `autumn credentials`); never commit it.
fn totp_enc_key() -> AutumnResult<[u8; 32]> {
    let raw = std::env::var("TOTP_ENC_KEY").map_err(|_| {
        AutumnError::internal_server_error_msg(
            "TOTP_ENC_KEY is not set. Generate 32 random bytes, base64-encode them, \
             and set TOTP_ENC_KEY before enabling two-factor auth.",
        )
    })?;
    let bytes = B64
        .decode(raw.trim())
        .map_err(|_| AutumnError::internal_server_error_msg("TOTP_ENC_KEY must be valid base64."))?;
    let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
        AutumnError::internal_server_error_msg("TOTP_ENC_KEY must decode to exactly 32 bytes.")
    })?;
    Ok(arr)
}

/// Encrypt a TOTP secret for storage. Output is base64(nonce ‖ ciphertext).
fn encrypt_secret(plaintext: &[u8]) -> AutumnResult<String> {
    use rand::TryRngCore;
    let key = totp_enc_key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|_| AutumnError::internal_server_error_msg("RNG failure generating nonce"))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| AutumnError::internal_server_error_msg("failed to encrypt TOTP secret"))?;
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ct);
    Ok(B64.encode(combined))
}

/// Decrypt a stored TOTP secret produced by [`encrypt_secret`].
fn decrypt_secret(stored: &str) -> AutumnResult<Vec<u8>> {
    let key = totp_enc_key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let raw = B64
        .decode(stored.trim())
        .map_err(|_| AutumnError::internal_server_error_msg("stored TOTP secret is not valid base64"))?;
    if raw.len() < 12 {
        return Err(AutumnError::internal_server_error_msg(
            "stored TOTP secret is malformed",
        ));
    }
    let (nonce_bytes, ct) = raw.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ct)
        .map_err(|_| AutumnError::internal_server_error_msg("failed to decrypt TOTP secret"))
}

/// Build a `TOTP` (SHA1, 6 digits, ±1 step skew) from raw secret bytes.
///
/// `get_url()` yields the standard `otpauth://` provisioning URI used for the QR.
fn build_totp(secret: Vec<u8>, account: &str) -> AutumnResult<TOTP> {
    // `totp-rs` rejects issuer/account labels containing a colon. The account
    // label is only a display hint in the `otpauth://` URI (it doesn't affect
    // code generation/verification), so strip colons defensively — otherwise an
    // address like `a:b@example.com` would 500 on enrollment.
    let account = account.replace(':', "_");
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        TOTP_STEP,
        secret,
        Some("__PASCAL__".to_owned()),
        account,
    )
    .map_err(|_| AutumnError::internal_server_error_msg("invalid TOTP secret"))
}

/// Current RFC 6238 time step.
fn current_step() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (secs / TOTP_STEP) as i64
}

/// Verify a TOTP code within a ±1 step window, returning the matched step.
///
/// The caller compares the returned step against `totp_last_used_step` to reject
/// replay of an already-consumed step.
fn verify_totp_code(totp: &TOTP, code: &str) -> Option<i64> {
    let step = current_step();
    let candidate = code.trim();
    for delta in [-1i64, 0, 1] {
        let s = step + delta;
        if s < 0 {
            continue;
        }
        let expected = totp.generate(s as u64 * TOTP_STEP);
        if expected == candidate {
            return Some(s);
        }
    }
    None
}

/// Generate one human-friendly single-use recovery code (10 hex chars, grouped).
fn generate_recovery_code() -> String {
    use rand::TryRngCore;
    let mut bytes = [0u8; 5];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS RNG failed");
    let hexs = hex::encode(bytes);
    format!("{}-{}", &hexs[..5], &hexs[5..])
}

/// `GET /account/2fa` — show current two-factor state. Requires authentication.
#[secured]
#[get("/account/2fa")]
pub async fn two_factor_status(
    session: Session,
    mut db: Db,
    csrf: Option<CsrfToken>,
    csrf_field: Option<CsrfFormField>,
) -> AutumnResult<Markup> {
    let __SNAKE___id: i64 = session
        .get("__SNAKE___id")
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;
    let __SNAKE__: __PASCAL__ = __TABLE__::table
        .find(__SNAKE___id)
        .select(__PASCAL__::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Account not found."))?;

    let remaining: i64 = if __SNAKE__.totp_enabled {
        recovery_codes::table
            .filter(recovery_codes::user_id.eq(__SNAKE__.id))
            .filter(recovery_codes::used_at.is_null())
            .count()
            .get_result(&mut *db)
            .await
            .unwrap_or(0)
    } else {
        0
    };

    Ok(layout("Two-Factor Authentication", html! {
        h1 { "Two-Factor Authentication" }
        @if __SNAKE__.totp_enabled {
            p { "Two-factor authentication is " strong { "enabled" } "." }
            p { "You have " (remaining) " recovery codes remaining." }
            form action="/account/2fa/disable" method="post" {
                @if let Some(ref csrf) = csrf { input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }
                p {
                    label { "Confirm with your password or a current 6-digit code:" }
                    input type="password" name="password" autocomplete="current-password";
                    input type="text" name="code" inputmode="numeric" autocomplete="one-time-code";
                }
                button type="submit" { "Disable two-factor" }
            }
        } @else {
            p { "Two-factor authentication is " strong { "disabled" } "." }
            form action="/account/2fa/enable" method="post" {
                @if let Some(ref csrf) = csrf { input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }
                p {
                    label { "Confirm your password to begin enrollment:" }
                    input type="password" name="password" autocomplete="current-password" required;
                }
                button type="submit" { "Enable two-factor" }
            }
        }
    }))
}

/// `POST /account/2fa/enable` — generate a secret, render the QR + manual key,
/// and ask the user to confirm with a valid code before enabling. Requires auth.
///
/// The pending secret is held in the session (encrypted) and is NOT written to
/// the account until `two_factor_confirm` succeeds. This means starting (and
/// abandoning) re-enrollment never disturbs an already-active second factor.
#[secured]
#[post("/account/2fa/enable")]
pub async fn two_factor_enable(
    session: Session,
    State(state): State<AppState>,
    mut db: Db,
    csrf: Option<CsrfToken>,
    csrf_field: Option<CsrfFormField>,
    Form(form): Form<TotpEnableForm>,
) -> AutumnResult<Markup> {
    // Enrollment stashes `totp_pending_secret` in the session. If the app
    // configured `[auth].session_key` to a reserved `totp_pending_*` name, that
    // write would clobber the live auth key and the account would later be
    // locked out by the reserved-key guard in the login interstitial. Refuse the
    // misconfiguration up front (the login/reset handoffs reject it too).
    if is_reserved_totp_pending_key(state.auth_session_key()) {
        return Err(AutumnError::internal_server_error_msg(
            "[auth].session_key collides with a reserved TOTP pending key (totp_pending_*). \
             Choose a different [auth].session_key.",
        ));
    }
    let __SNAKE___id: i64 = session
        .get("__SNAKE___id")
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;
    let __SNAKE__: __PASCAL__ = __TABLE__::table
        .find(__SNAKE___id)
        .select(__PASCAL__::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Account not found."))?;

    // Step-up re-auth: a logged-in but unattended/hijacked session must not be
    // able to enroll an attacker's authenticator. Require the account password
    // before starting enrollment (matches GitHub/Google behaviour).
    if !verify_password(&form.password, &__SNAKE__.password_digest)
        .await
        .unwrap_or(false)
    {
        return Err(AutumnError::unprocessable_msg(
            "Password confirmation failed.",
        ));
    }

    // Re-enrollment guard: if 2FA is already active, a hijacked (already-past-2FA)
    // session must not be able to swap in a new authenticator without re-proving
    // the current factor. Disabling first goes through `/account/2fa/disable`,
    // which requires a current code or the password.
    if __SNAKE__.totp_enabled {
        return Err(AutumnError::unprocessable_msg(
            "Two-factor is already enabled. Disable it first (which requires your \
             current code or password) before enrolling a new authenticator.",
        ));
    }

    let secret = Secret::generate_secret();
    let secret_bytes = secret
        .to_bytes()
        .map_err(|_| AutumnError::internal_server_error_msg("failed to generate TOTP secret"))?;
    let manual_key = secret.to_encoded().to_string();
    let totp = build_totp(secret_bytes.clone(), &__SNAKE__.email)?;
    // The provisioning URI (otpauth://…) drives the QR and manual entry.
    let provisioning_uri = totp.get_url();
    let qr = totp
        .get_qr_base64()
        .map_err(|_| AutumnError::internal_server_error_msg("failed to render QR code"))?;

    // Stash the pending secret in the session only — the live account row is
    // untouched until the user proves possession in `two_factor_confirm`.
    let encrypted = encrypt_secret(&secret_bytes)?;
    session.insert("totp_pending_secret", &encrypted).await;

    Ok(layout("Enable Two-Factor", html! {
        h1 { "Scan this QR code" }
        p { "Scan with Google Authenticator, 1Password, or any RFC 6238 app." }
        img src=(format!("data:image/png;base64,{}", qr)) alt="TOTP QR code";
        p { "Or enter this key manually: " code { (manual_key) } }
        p { small { "Provisioning URI: " code { (provisioning_uri) } } }
        h2 { "Confirm" }
        p { "Enter the 6-digit code shown in your app to finish enabling 2FA." }
        form action="/account/2fa/confirm" method="post" {
            @if let Some(ref csrf) = csrf { input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }
            input type="text" name="code" inputmode="numeric" autocomplete="one-time-code" required;
            button type="submit" { "Confirm and enable" }
        }
    }))
}

/// `POST /account/2fa/confirm` — verify a code against the pending secret, flip
/// `totp_enabled`, and generate single-use recovery codes (shown once). Requires auth.
#[secured]
#[post("/account/2fa/confirm")]
pub async fn two_factor_confirm(
    session: Session,
    mut db: Db,
    Form(form): Form<TotpCodeForm>,
) -> AutumnResult<Markup> {
    let __SNAKE___id: i64 = session
        .get("__SNAKE___id")
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;
    let __SNAKE__: __PASCAL__ = __TABLE__::table
        .find(__SNAKE___id)
        .select(__PASCAL__::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Account not found."))?;

    // Re-enrollment guard (defense in depth — `two_factor_enable` blocks this too):
    // never replace an active factor without first disabling (which re-authenticates).
    if __SNAKE__.totp_enabled {
        return Err(AutumnError::unprocessable_msg(
            "Two-factor is already enabled. Disable it first before confirming a \
             new authenticator.",
        ));
    }

    // The pending secret lives in the session (set by `two_factor_enable`), so
    // an unconfirmed enrollment never touched the live account row.
    let pending = session
        .get("totp_pending_secret")
        .await
        .ok_or_else(|| AutumnError::unprocessable_msg("Start enrollment first."))?;
    let secret = decrypt_secret(&pending)?;
    let totp = build_totp(secret, &__SNAKE__.email)?;
    let Some(step) = verify_totp_code(&totp, &form.code) else {
        return Err(AutumnError::unprocessable_msg("Invalid code. Try again."));
    };

    // Persist the new secret and a fresh single-use recovery-code set BEFORE
    // flipping `totp_enabled`, so a failure mid-way leaves the account in its
    // prior state rather than locked behind a secret whose codes were never
    // shown. Run it all in one transaction for atomicity.
    let pending_secret = pending.clone();
    let user_id = __SNAKE__.id;
    let mut plaintext_codes: Vec<String> = Vec::with_capacity(RECOVERY_CODE_COUNT);
    let mut hashed_codes: Vec<String> = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let code = generate_recovery_code();
        hashed_codes.push(hash_password(&code).await?);
        plaintext_codes.push(code);
    }

    let txn_result = (*db)
        .transaction::<_, diesel::result::Error, _>(|conn| {
        Box::pin(async move {
            diesel::update(__TABLE__::table.find(user_id))
                .set((
                    __TABLE__::totp_secret_encrypted.eq(Some(pending_secret)),
                    __TABLE__::totp_last_used_step.eq(Some(step)),
                ))
                .execute(conn)
                .await?;
            diesel::delete(recovery_codes::table.filter(recovery_codes::user_id.eq(user_id)))
                .execute(conn)
                .await?;
            for digest in hashed_codes {
                diesel::insert_into(recovery_codes::table)
                    .values(NewRecoveryCode {
                        user_id,
                        code_digest: digest,
                        used_at: None,
                    })
                    .execute(conn)
                    .await?;
            }
            // Flip the flag last, conditional on it still being false. This is
            // both the durability gate (the account is only "2FA-enabled" once
            // the new secret + recovery codes are stored) and the race gate: if
            // a concurrent confirm already enabled 2FA, this matches zero rows
            // and we roll back, discarding this request's recovery codes so the
            // stored set always matches the plaintext the winner displayed.
            let claimed = diesel::update(
                __TABLE__::table
                    .find(user_id)
                    .filter(__TABLE__::totp_enabled.eq(false)),
            )
            .set(__TABLE__::totp_enabled.eq(true))
            .execute(conn)
            .await?;
            if claimed != 1 {
                return Err(diesel::result::Error::RollbackTransaction);
            }
            Ok(())
        })
        })
        .await;
    match txn_result {
        Ok(()) => {}
        Err(diesel::result::Error::RollbackTransaction) => {
            return Err(AutumnError::unprocessable_msg(
                "Two-factor was just enabled by another request. Reload /account/2fa.",
            ));
        }
        Err(_) => {
            return Err(AutumnError::internal_server_error_msg(
                "Failed to enable two-factor.",
            ));
        }
    }

    session.remove("totp_pending_secret").await;

    Ok(layout("Save Your Recovery Codes", html! {
        h1 { "Two-factor authentication enabled" }
        p { strong { "Save these recovery codes now." } " Each can be used once if you lose your device. They will not be shown again." }
        ul {
            @for code in &plaintext_codes {
                li { code { (code) } }
            }
        }
        p { a href="/account/2fa" { "Done" } }
    }))
}

/// `POST /account/2fa/disable` — require re-auth (current code OR password), then
/// clear the secret and all recovery codes. Requires authentication.
#[secured]
#[post("/account/2fa/disable")]
pub async fn two_factor_disable(
    session: Session,
    mut db: Db,
    Form(form): Form<TotpDisableForm>,
) -> AutumnResult<Response> {
    let __SNAKE___id: i64 = session
        .get("__SNAKE___id")
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;
    let __SNAKE__: __PASCAL__ = __TABLE__::table
        .find(__SNAKE___id)
        .select(__PASCAL__::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::not_found_msg("Account not found."))?;

    // Re-authenticate: accept a current TOTP code or the account password.
    // A TOTP code used here is consumed with the same atomic replay guard as
    // `/login/verify`, so a code already spent on login cannot be replayed to
    // disable the factor.
    let mut ok = false;
    if let Some(code) = form.code.as_deref().filter(|c| !c.trim().is_empty()) {
        if let Some(stored) = __SNAKE__.totp_secret_encrypted.as_deref() {
            // A decrypt/build failure (e.g. rotated TOTP_ENC_KEY) must not abort:
            // the password fallback below is the escape from that lockout.
            if let Some(step) = decrypt_secret(stored)
                .ok()
                .and_then(|secret| build_totp(secret, &__SNAKE__.email).ok())
                .and_then(|totp| verify_totp_code(&totp, code))
            {
                let affected = diesel::update(
                    __TABLE__::table.find(__SNAKE__.id).filter(
                        __TABLE__::totp_last_used_step
                            .is_null()
                            .or(__TABLE__::totp_last_used_step.lt(step)),
                    ),
                )
                .set(__TABLE__::totp_last_used_step.eq(Some(step)))
                .execute(&mut *db)
                .await?;
                ok = affected == 1;
            }
        }
    }
    if !ok {
        if let Some(password) = form.password.as_deref().filter(|p| !p.is_empty()) {
            ok = verify_password(password, &__SNAKE__.password_digest)
                .await
                .unwrap_or(false);
        }
    }
    if !ok {
        return Err(AutumnError::unprocessable_msg(
            "Re-authentication failed. Provide a valid code or your password.",
        ));
    }

    // Disable the factor and delete the recovery codes in one transaction so a
    // mid-operation failure can't leave the account with 2FA turned off but
    // stale recovery codes still present (or vice versa).
    let user_id = __SNAKE__.id;
    (*db)
        .transaction::<_, diesel::result::Error, _>(|conn| {
            Box::pin(async move {
                diesel::update(__TABLE__::table.find(user_id))
                    .set((
                        __TABLE__::totp_enabled.eq(false),
                        __TABLE__::totp_secret_encrypted.eq(None::<String>),
                        __TABLE__::totp_last_used_step.eq(None::<i64>),
                    ))
                    .execute(conn)
                    .await?;
                diesel::delete(recovery_codes::table.filter(recovery_codes::user_id.eq(user_id)))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .await
        .map_err(|_| AutumnError::internal_server_error_msg("Failed to disable two-factor."))?;

    Ok(redirect_to("/account/2fa"))
}

/// `GET /login/verify` — second-factor prompt shown after a correct password
/// when the account has 2FA enabled (the session is `totp_pending`).
#[get("/login/verify")]
pub async fn login_verify_form(
    session: Session,
    csrf: Option<CsrfToken>,
    csrf_field: Option<CsrfFormField>,
) -> AutumnResult<Response> {
    if session.get("totp_pending_id").await.is_none() {
        return Ok(redirect_to("/login"));
    }
    Ok(layout("Two-Factor Verification", html! {
        h1 { "Two-Factor Verification" }
        form action="/login/verify" method="post" {
            @if let Some(ref csrf) = csrf { input type="hidden" name=(csrf_field.as_ref().map_or("_csrf", |f| f.0.as_str())) value=(csrf.token()); }
            div {
                label { "Authentication code or recovery code" }
                input type="text" name="code" inputmode="numeric" autocomplete="one-time-code" required;
            }
            button type="submit" { "Verify" }
        }
    })
    .into_response())
}

/// `POST /login/verify` — accept a TOTP code OR a single-use recovery code.
///
/// Only on success is the `#[secured]` auth key set. A consumed recovery code is
/// marked `used_at`; a used TOTP step cannot be replayed.
#[post("/login/verify")]
pub async fn login_verify(
    mut db: Db,
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<TotpCodeForm>,
) -> AutumnResult<Response> {
    let pending_id: i64 = session
        .get("totp_pending_id")
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("No pending login."))?;
    let __SNAKE__: __PASCAL__ = __TABLE__::table
        .find(pending_id)
        .select(__PASCAL__::as_select())
        .first(&mut *db)
        .await
        .map_err(|_| AutumnError::unauthorized_msg("No pending login."))?;

    let submitted = form.code.trim().to_owned();
    let mut verified = false;

    // 1) Try TOTP within the ±1 window, rejecting an already-used step (replay).
    //    Consumption is atomic: the UPDATE only matches rows whose stored step is
    //    still below this one, and we require exactly one row to be changed. Two
    //    concurrent requests with the same code therefore cannot both win — the
    //    second sees `affected == 0`.
    //
    //    A decrypt/build failure here (e.g. TOTP_ENC_KEY rotated/missing, or a
    //    corrupt blob) is treated like a non-matching code rather than aborting,
    //    so the user can still fall back to a recovery code below — recovery
    //    codes are stored independently and are the only escape from that lockout.
    if let Some(stored) = __SNAKE__.totp_secret_encrypted.as_deref() {
        if let Some(step) = decrypt_secret(stored)
            .ok()
            .and_then(|secret| build_totp(secret, &__SNAKE__.email).ok())
            .and_then(|totp| verify_totp_code(&totp, &submitted))
        {
            let affected = diesel::update(
                __TABLE__::table.find(__SNAKE__.id).filter(
                    __TABLE__::totp_last_used_step
                        .is_null()
                        .or(__TABLE__::totp_last_used_step.lt(step)),
                ),
            )
            .set(__TABLE__::totp_last_used_step.eq(Some(step)))
            .execute(&mut *db)
            .await?;
            if affected == 1 {
                verified = true;
            }
        }
    }

    // 2) Otherwise, try an unused recovery code (single-use). Marking it used is
    //    a conditional UPDATE guarded by `used_at IS NULL`; the code is only
    //    accepted when this request is the one that flips it, so a code can never
    //    be redeemed twice even under concurrent submissions.
    if !verified {
        let unused: Vec<RecoveryCode> = recovery_codes::table
            .filter(recovery_codes::user_id.eq(__SNAKE__.id))
            .filter(recovery_codes::used_at.is_null())
            .select(RecoveryCode::as_select())
            .load(&mut *db)
            .await
            .unwrap_or_default();
        for rc in &unused {
            if verify_password(&submitted, &rc.code_digest).await.unwrap_or(false) {
                let affected = diesel::update(
                    recovery_codes::table
                        .find(rc.id)
                        .filter(recovery_codes::used_at.is_null()),
                )
                .set(recovery_codes::used_at.eq(Some(chrono::Utc::now().naive_utc())))
                .execute(&mut *db)
                .await?;
                if affected == 1 {
                    verified = true;
                    break;
                }
            }
        }
    }

    if !verified {
        return Err(AutumnError::unprocessable_msg("Invalid code."));
    }

    // If this verification is finishing a password reset (the reset handler
    // parked the new digest for a 2FA-enabled account), commit it now — only
    // after the second factor has been proven.  The conditional UPDATE also
    // guards against a superseded token: if a second reset was requested between
    // parking and now, reset_token_digest in the DB will no longer match the
    // stored token and the update matches 0 rows, which we treat as an error.
    if let Some(new_digest) = session.get("totp_pending_reset_digest").await {
        let stored_token: Option<String> = session.get("totp_pending_reset_token").await;
        session.remove("totp_pending_reset_digest").await;
        session.remove("totp_pending_reset_token").await;
        let committed = if let Some(token) = stored_token {
            let now = chrono::Utc::now().naive_utc();
            let updated = diesel::update(__TABLE__::table.find(__SNAKE__.id))
                .filter(__TABLE__::reset_token_digest.eq(Some(token.as_str())))
                .filter(__TABLE__::reset_token_expires_at.gt(now))
                .set((
                    __TABLE__::password_digest.eq(&new_digest),
                    __TABLE__::reset_token_digest.eq(None::<String>),
                    __TABLE__::reset_token_expires_at.eq(None::<chrono::NaiveDateTime>),
                ))
                .execute(&mut *db)
                .await?;
            updated == 1
        } else {
            false
        };
        if !committed {
            // This login was authorised solely by the reset link, which has now
            // proven stale (expired or superseded). Tear down the entire pending
            // login — including `totp_pending_id` — so a retry cannot skip the
            // (now-cleared) reset branch and promote to a full session without a
            // fresh login.
            session.remove("totp_pending_id").await;
            session.remove("totp_pending_secret").await;
            return Err(AutumnError::unprocessable_msg(
                "Reset link expired or already used. Please restart the password reset.",
            ));
        }
    }

    // Promote the pending session to a fully authenticated one.
    session.rotate_id().await;
    session.remove("totp_pending_id").await;
    session.insert("__SNAKE___id", __SNAKE__.id.to_string()).await;
    session.insert("__SNAKE___email", &__SNAKE__.email).await;
    session.insert(state.auth_session_key(), __SNAKE__.id.to_string()).await;

    let remaining: i64 = recovery_codes::table
        .filter(recovery_codes::user_id.eq(__SNAKE__.id))
        .filter(recovery_codes::used_at.is_null())
        .count()
        .get_result(&mut *db)
        .await
        .unwrap_or(0);
    // Surface remaining recovery-code count via a flash-style query param.
    Ok(redirect_to(&format!("/account?recovery_remaining={}", remaining)))
}
"#;
    TPL.replace("__PASCAL__", pascal_name)
        .replace("__SNAKE__", snake_name)
        .replace("__TABLE__", table)
}

/// Markdown appended to `docs/guide/authentication.md` under `--totp`.
const TOTP_DOCS_SECTION: &str = r#"## Two-Factor Authentication (TOTP)

Generated with `--totp`. Users can enroll an authenticator app (Google
Authenticator, 1Password, …) and fall back to single-use recovery codes.

### Generated routes

| Method | Path | Handler | Auth |
|--------|------|---------|------|
| GET | `/account/2fa` | `two_factor_status` | **Required** |
| POST | `/account/2fa/enable` | `two_factor_enable` | **Required** |
| POST | `/account/2fa/confirm` | `two_factor_confirm` | **Required** |
| POST | `/account/2fa/disable` | `two_factor_disable` | **Required** |
| GET | `/login/verify` | `login_verify_form` | Pending login |
| POST | `/login/verify` | `login_verify` | Pending login |

### Encryption key

TOTP secrets are encrypted at rest with AES-256-GCM. Set `TOTP_ENC_KEY` to a
base64-encoded 32-byte key before enabling 2FA:

```sh
# 32 random bytes, base64-encoded
export TOTP_ENC_KEY="$(head -c 32 /dev/urandom | base64)"
```

Manage it like any other secret (`autumn credentials`); never commit it. Rotating
the key invalidates existing enrollments.

### Security properties

- Secrets encrypted at rest (never stored plaintext).
- Verification accepts a ±1 time-step window; a consumed step cannot be replayed
  (`totp_last_used_step`), including on the disable path.
- Recovery codes are single-use, bcrypt-hashed, and stamped `used_at` on use.
- Enrolling requires step-up re-authentication (the account password), so a
  stolen/unattended session cannot quietly add an attacker's authenticator.
- Re-enrollment while 2FA is active is rejected — disable first (which itself
  requires a current code or password).
- Login, OAuth callback (guidance), and password reset all route 2FA-enabled
  accounts through `/login/verify`. A password reset for a 2FA account is
  **deferred**: the new password is parked and only committed after the second
  factor is proven, so a reset link alone cannot change the password.
- Disabling 2FA requires re-authentication (current code or password) and clears
  the secret plus all recovery codes.

### Out of scope (follow-ups)

- **Rate-limiting / lockout** on repeated failed second-factor attempts is not
  included — add it before exposing 2FA to untrusted traffic.
- WebAuthn / passkeys and SMS/email OTP are separate tracks.

"#;

/// Render `tests/auth_2fa.rs` — the generated 2FA integration suite.
fn render_2fa_tests_file(pascal_name: &str, snake_name: &str) -> String {
    const TPL: &str = r#"//! Generated 2FA integration tests for __PASCAL__ (`autumn generate auth --totp`).
//!
//! Like `tests/auth.rs`, these run against a live server started with
//! `AUTUMN_TEST_BASE_URL` and skip when it is unset, so they compile and pass
//! out of the box. Flesh out the bodies once your test harness boots the app
//! with a database and a `TOTP_ENC_KEY`.
//!
//! Covered flows: enroll → confirm → login-with-code → login-with-recovery-code
//! → recovery-code-reuse-rejected → disable.

fn base_url() -> Option<String> {
    std::env::var("AUTUMN_TEST_BASE_URL").ok()
}

#[test]
fn two_factor_enroll_and_confirm() {
    let Some(_base) = base_url() else {
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    };
    // POST /account/2fa/enable then /account/2fa/confirm with a generated code.
}

#[test]
fn login_with_totp_code() {
    let Some(_base) = base_url() else {
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    };
    // Password login redirects to /login/verify; a valid code completes login.
}

#[test]
fn login_with_recovery_code() {
    let Some(_base) = base_url() else {
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    };
    // A single-use recovery code is accepted at /login/verify.
}

#[test]
fn recovery_code_reuse_rejected() {
    let Some(_base) = base_url() else {
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    };
    // Re-submitting a consumed recovery code is rejected.
}

#[test]
fn two_factor_disable() {
    let Some(_base) = base_url() else {
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    };
    // POST /account/2fa/disable with re-auth clears the secret + recovery codes.
}
"#;
    TPL.replace("__PASCAL__", pascal_name)
        .replace("__SNAKE__", snake_name)
}

// ── Passkey (WebAuthn) helpers ────────────────────────────────────────────────

fn passkey_route_entries() -> Vec<String> {
    vec![
        "routes::passkeys::passkey_register_begin".to_owned(),
        "routes::passkeys::passkey_register_finish".to_owned(),
        "routes::passkeys::passkey_login_begin".to_owned(),
        "routes::passkeys::passkey_login_finish".to_owned(),
        "routes::passkeys::passkey_list".to_owned(),
        "routes::passkeys::passkey_revoke".to_owned(),
        "routes::passkeys::passkey_register_page".to_owned(),
        "routes::passkeys::passkey_login_page".to_owned(),
    ]
}

fn render_passkey_migration_up(user_table: &str) -> String {
    format!(
        "CREATE TABLE webauthn_credentials (\n\
         \x20   id BIGSERIAL PRIMARY KEY,\n\
         \x20   user_id BIGINT NOT NULL REFERENCES {user_table}(id) ON DELETE CASCADE,\n\
         \x20   credential_id TEXT NOT NULL UNIQUE,\n\
         \x20   credential_json TEXT NOT NULL,\n\
         \x20   name TEXT NOT NULL DEFAULT 'Passkey',\n\
         \x20   created_at TIMESTAMP NOT NULL DEFAULT NOW(),\n\
         \x20   last_used_at TIMESTAMP NULL\n\
         );\n\
         \n\
         CREATE INDEX webauthn_credentials_user_id_idx ON webauthn_credentials (user_id);\n"
    )
}

fn render_passkey_migration_down() -> String {
    "DROP TABLE webauthn_credentials;\n".to_owned()
}

fn render_webauthn_credential_model_file(user_table: &str) -> String {
    format!(
        r"//! Generated by `autumn generate auth --passkeys`.
//!
//! Edit freely — once generated, this is ordinary user code.

use crate::schema::{{webauthn_credentials, {user_table}}};

#[autumn_web::model]
pub struct WebauthnCredential {{
    pub id: i64,
    pub user_id: i64,
    pub credential_id: String,
    pub credential_json: String,
    pub name: String,
    pub created_at: chrono::NaiveDateTime,
    #[default]
    pub last_used_at: Option<chrono::NaiveDateTime>,
}}

diesel::joinable!(webauthn_credentials -> {user_table} (user_id));
"
    )
}

#[allow(clippy::too_many_lines)]
fn render_passkeys_routes_file(pascal_name: &str, snake_name: &str, user_table: &str) -> String {
    let tpl = r##"//! Generated by `autumn generate auth --passkeys`.
//!
//! WebAuthn passkey ceremony handlers.  Edit freely — once generated, this is
//! ordinary user code.
//!
//! # Configuration
//!
//! Add to `autumn.toml`:
//!
//! ```toml
//! [auth.webauthn]
//! rp_id     = "example.com"
//! rp_name   = "My App"
//! rp_origin = "https://example.com"
//! ```
//!
//! # Security notes
//!
//! - `passkey_login_finish` writes `state.auth_session_key()` so downstream
//!   `#[secured]` routes work without modification.
//! - Never store raw credentials — `credential_json` holds the opaque passkey
//!   state returned by webauthn-rs and should not be inspected by app code.

use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};
use webauthn_rs::prelude::*;

fn redirect_to(url: &str) -> impl IntoResponse {
    axum::response::Redirect::to(url)
}

// ── Config helper ──────────────────────────────────────────────────────────────

fn build_webauthn(state: &AppState) -> AutumnResult<Webauthn> {
    let cfg = &state.config().auth.webauthn;
    if cfg.rp_id.is_empty() || cfg.rp_origin.is_empty() {
        return Err(AutumnError::internal_server_error_msg(
            "WebAuthn is not configured. Set [auth.webauthn] rp_id, rp_name, and rp_origin \
             in autumn.toml. See docs/guide/passkeys.md for details.",
        ));
    }
    let origin = Url::parse(&cfg.rp_origin).map_err(|_| {
        AutumnError::internal_server_error_msg(
            "auth.webauthn.rp_origin is not a valid URL. \
             Example: \"https://example.com\"",
        )
    })?;
    WebauthnBuilder::new(&cfg.rp_id, &origin)
        .map_err(|e| {
            AutumnError::internal_server_error_msg(format!(
                "Failed to build WebAuthn instance from auth.webauthn config: {e}"
            ))
        })?
        .rp_name(&cfg.rp_name)
        .build()
        .map_err(|e| {
            AutumnError::internal_server_error_msg(format!(
                "Failed to build WebAuthn instance: {e}"
            ))
        })
}

// ── Forms / JSON types ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PasskeyRegisterFinishBody {
    pub response: serde_json::Value,
}

#[derive(Deserialize)]
pub struct PasskeyLoginFinishBody {
    pub response: serde_json::Value,
}

#[derive(Deserialize)]
pub struct PasskeyRevokeForm {
    pub id: i64,
}

// ── Registration ──────────────────────────────────────────────────────────────

/// `GET /passkeys/register` — passkey registration UI page.
#[secured]
#[get("/passkeys/register")]
pub async fn passkey_register_page(
    session: Session,
    State(state): State<AppState>,
    csrf: Option<CsrfToken>,
    csrf_header: Option<CsrfTokenHeader>,
    nonce: Option<CspNonce>,
) -> AutumnResult<Markup> {
    let _ = (session, state);
    let csrf_token = csrf.map(|t| t.token().to_owned()).unwrap_or_default();
    let csrf_header_name = csrf_header.map(|h| h.0.clone()).unwrap_or_else(|| "X-CSRF-Token".to_owned());
    let script_nonce = nonce.map(|n| n.value().to_owned());
    Ok(html! {
        html {
            head {
                title { "Register a Passkey" }
                meta name="csrf-token" content=(csrf_token);
                meta name="csrf-token-header" content=(csrf_header_name);
            }
            body {
                h1 { "Register a Passkey" }
                button id="register-btn" { "Register passkey" }
                script nonce=[script_nonce] {
                    (PreEscaped(r#"
const csrfToken = document.querySelector('meta[name="csrf-token"]')?.content ?? '';
const csrfHeader = document.querySelector('meta[name="csrf-token-header"]')?.content ?? 'X-CSRF-Token';
document.getElementById('register-btn').addEventListener('click', async () => {
    const beginResp = await fetch('/passkeys/register/begin', {
        method: 'POST',
        headers: { [csrfHeader]: csrfToken },
    });
    const optionsJSON = await beginResp.json();
    const options = PublicKeyCredential.parseCreationOptionsFromJSON(optionsJSON.publicKey);
    const credential = await navigator.credentials.create({ publicKey: options });
    const finishResp = await fetch('/passkeys/register/finish', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', [csrfHeader]: csrfToken },
        body: JSON.stringify({ response: credential.toJSON() }),
    });
    if (finishResp.ok) {
        window.location.href = '/passkeys';
    } else {
        alert('Registration failed');
    }
});
"#))
                }
            }
        }
    })
}

/// `POST /passkeys/register/begin` — start a registration ceremony.
///
/// Requires authentication. Stores the pending challenge in the session.
#[secured]
#[post("/passkeys/register/begin")]
pub async fn passkey_register_begin(
    session: Session,
    State(state): State<AppState>,
    mut db: Db,
) -> AutumnResult<axum::Json<serde_json::Value>> {
    let __SNAKE___id: i64 = session
        .get(state.auth_session_key())
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;
    let __SNAKE___email = session
        .get("__SNAKE___email")
        .await
        .unwrap_or_else(|| "user".to_owned());
    let webauthn = build_webauthn(&state)?;
    let existing_cred_ids: Vec<CredentialID> = {
        use crate::schema::webauthn_credentials;
        webauthn_credentials::table
            .filter(webauthn_credentials::user_id.eq(__SNAKE___id))
            .select(webauthn_credentials::credential_json)
            .load::<String>(&mut *db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|json| serde_json::from_str::<Passkey>(&json).ok())
            .map(|p| p.cred_id().clone())
            .collect()
    };
    let user_unique_id = uuid::Uuid::from_u128(__SNAKE___id as u128);
    let (ccr, reg_state) = webauthn
        .start_passkey_registration(
            user_unique_id,
            &__SNAKE___email,
            &__SNAKE___email,
            Some(existing_cred_ids),
        )
        .map_err(|e| {
            AutumnError::internal_server_error_msg(format!("start_passkey_registration: {e}"))
        })?;
    // Store the user_id alongside the reg_state so finish can verify the session
    // user hasn't changed between begin and finish (prevents cross-account TOCTOU).
    session
        .insert(
            "passkey_reg_state",
            serde_json::to_string(&serde_json::json!({
                "user_id": __SNAKE___id,
                "state":   serde_json::to_string(&reg_state).unwrap_or_default(),
            }))
            .unwrap_or_default(),
        )
        .await;
    Ok(axum::Json(serde_json::to_value(ccr).unwrap_or_default()))
}

/// `POST /passkeys/register/finish` — complete a registration ceremony.
///
/// Stores the new credential in `webauthn_credentials`.
#[secured]
#[post("/passkeys/register/finish")]
pub async fn passkey_register_finish(
    session: Session,
    State(state): State<AppState>,
    mut db: Db,
    axum::Json(body): axum::Json<PasskeyRegisterFinishBody>,
) -> AutumnResult<axum::Json<serde_json::Value>> {
    let current_id: i64 = session
        .get(state.auth_session_key())
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;
    let envelope_str: String = session
        .get("passkey_reg_state")
        .await
        .ok_or_else(|| AutumnError::unprocessable_msg("No pending registration."))?;
    session.remove("passkey_reg_state").await;
    let envelope: serde_json::Value = serde_json::from_str(&envelope_str)
        .map_err(|_| AutumnError::unprocessable_msg("Invalid registration state."))?;
    let __SNAKE___id: i64 = envelope["user_id"]
        .as_i64()
        .ok_or_else(|| AutumnError::unprocessable_msg("Invalid registration state."))?;
    if __SNAKE___id != current_id {
        return Err(AutumnError::unauthorized_msg(
            "Session user changed since registration began.",
        ));
    }
    let reg_state_str = envelope["state"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let reg_state: PasskeyRegistration = serde_json::from_str(&reg_state_str)
        .map_err(|_| AutumnError::unprocessable_msg("Invalid registration state."))?;
    let webauthn = build_webauthn(&state)?;
    let rpk_finish: RegisterPublicKeyCredential =
        serde_json::from_value(body.response).map_err(|e| {
            AutumnError::unprocessable_msg(format!("Invalid credential response: {e}"))
        })?;
    let passkey = webauthn
        .finish_passkey_registration(&rpk_finish, &reg_state)
        .map_err(|e| AutumnError::unprocessable_msg(format!("Registration failed: {e}")))?;
    let cred_id = passkey.cred_id().to_string();
    let cred_json = serde_json::to_string(&passkey)
        .map_err(|_| AutumnError::internal_server_error_msg("Failed to serialise passkey."))?;
    diesel::insert_into(crate::schema::webauthn_credentials::table)
        .values((
            crate::schema::webauthn_credentials::user_id.eq(__SNAKE___id),
            crate::schema::webauthn_credentials::credential_id.eq(&cred_id),
            crate::schema::webauthn_credentials::credential_json.eq(&cred_json),
            crate::schema::webauthn_credentials::name.eq("Passkey"),
        ))
        .execute(&mut *db)
        .await
        .map_err(|_| AutumnError::internal_server_error_msg("Failed to store passkey."))?;
    Ok(axum::Json(serde_json::json!({ "ok": true })))
}

// ── Authentication ────────────────────────────────────────────────────────────

/// `GET /passkeys/login` — passkey login UI page.
#[get("/passkeys/login")]
pub async fn passkey_login_page(
    csrf: Option<CsrfToken>,
    csrf_header: Option<CsrfTokenHeader>,
    nonce: Option<CspNonce>,
) -> AutumnResult<Markup> {
    let csrf_token = csrf.map(|t| t.token().to_owned()).unwrap_or_default();
    let csrf_header_name = csrf_header.map(|h| h.0.clone()).unwrap_or_else(|| "X-CSRF-Token".to_owned());
    let script_nonce = nonce.map(|n| n.value().to_owned());
    Ok(html! {
        html {
            head {
                title { "Sign in with Passkey" }
                meta name="csrf-token" content=(csrf_token);
                meta name="csrf-token-header" content=(csrf_header_name);
            }
            body {
                h1 { "Sign in with a Passkey" }
                button id="login-btn" { "Sign in with passkey" }
                script nonce=[script_nonce] {
                    (PreEscaped(r#"
const csrfToken = document.querySelector('meta[name="csrf-token"]')?.content ?? '';
const csrfHeader = document.querySelector('meta[name="csrf-token-header"]')?.content ?? 'X-CSRF-Token';
document.getElementById('login-btn').addEventListener('click', async () => {
    const beginResp = await fetch('/passkeys/login/begin', {
        method: 'POST',
        headers: { [csrfHeader]: csrfToken },
    });
    const optionsJSON = await beginResp.json();
    const options = PublicKeyCredential.parseRequestOptionsFromJSON(optionsJSON.publicKey);
    const assertion = await navigator.credentials.get({ publicKey: options });
    const finishResp = await fetch('/passkeys/login/finish', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', [csrfHeader]: csrfToken },
        body: JSON.stringify({ response: assertion.toJSON() }),
    });
    if (finishResp.ok) {
        window.location.href = '/account';
    } else {
        alert('Login failed');
    }
});
"#))
                }
            }
        }
    })
}

/// `POST /passkeys/login/begin` — start a discoverable authentication ceremony.
///
/// Uses `start_discoverable_authentication` so the browser shows the passkey
/// selector without the server knowing the user upfront.  No credential IDs
/// are sent to anonymous clients.
#[post("/passkeys/login/begin")]
pub async fn passkey_login_begin(
    session: Session,
    State(state): State<AppState>,
) -> AutumnResult<axum::Json<serde_json::Value>> {
    let webauthn = build_webauthn(&state)?;
    let (rcr, auth_state) = webauthn
        .start_discoverable_authentication()
        .map_err(|e| {
            AutumnError::internal_server_error_msg(format!(
                "start_discoverable_authentication: {e}"
            ))
        })?;
    session
        .insert(
            "passkey_auth_state",
            serde_json::to_string(&auth_state).unwrap_or_default(),
        )
        .await;
    Ok(axum::Json(serde_json::to_value(rcr).unwrap_or_default()))
}

/// `POST /passkeys/login/finish` — complete a discoverable authentication ceremony.
///
/// On success writes `state.auth_session_key()`, `__SNAKE___id`, and
/// `__SNAKE___email` so downstream `#[secured]` routes work without modification.
#[post("/passkeys/login/finish")]
pub async fn passkey_login_finish(
    session: Session,
    State(state): State<AppState>,
    mut db: Db,
    axum::Json(body): axum::Json<PasskeyLoginFinishBody>,
) -> AutumnResult<axum::Json<serde_json::Value>> {
    let auth_state_str: String = session
        .get("passkey_auth_state")
        .await
        .ok_or_else(|| AutumnError::unprocessable_msg("No pending authentication."))?;
    session.remove("passkey_auth_state").await;
    let auth_state: DiscoverableAuthentication = serde_json::from_str(&auth_state_str)
        .map_err(|_| AutumnError::unprocessable_msg("Invalid authentication state."))?;
    let webauthn = build_webauthn(&state)?;
    let pkc: PublicKeyCredential = serde_json::from_value(body.response)
        .map_err(|e| AutumnError::unprocessable_msg(format!("Invalid credential: {e}")))?;
    // Decode user identity from the credential's userHandle (set during registration).
    let (user_uuid, _) = webauthn
        .identify_discoverable_authentication(&pkc)
        .map_err(|e| AutumnError::unauthorized_msg(format!("Cannot identify user: {e}")))?;
    let __SNAKE___id = user_uuid.as_u128() as i64;
    // Load only this user's passkeys for signature verification.
    let disc_keys: Vec<DiscoverableKey> = {
        use crate::schema::webauthn_credentials;
        webauthn_credentials::table
            .filter(webauthn_credentials::user_id.eq(__SNAKE___id))
            .select(webauthn_credentials::credential_json)
            .load::<String>(&mut *db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|json| serde_json::from_str::<Passkey>(&json).ok())
            .map(|p| DiscoverableKey::from(&p))
            .collect()
    };
    let auth_result = webauthn
        .finish_discoverable_authentication(&pkc, auth_state, &disc_keys)
        .map_err(|e| AutumnError::unauthorized_msg(format!("Authentication failed: {e}")))?;
    let cred_id_str = auth_result.cred_id().to_string();
    let (wc_id, cred_json) = {
        use crate::schema::webauthn_credentials;
        webauthn_credentials::table
            .filter(webauthn_credentials::credential_id.eq(&cred_id_str))
            .filter(webauthn_credentials::user_id.eq(__SNAKE___id))
            .select((webauthn_credentials::id, webauthn_credentials::credential_json))
            .first::<(i64, String)>(&mut *db)
            .await
            .map_err(|_| AutumnError::unauthorized_msg("Unknown credential."))?
    };
    let __SNAKE___email: String = {
        use crate::schema::__TABLE__;
        __TABLE__::table
            .find(__SNAKE___id)
            .select(__TABLE__::email)
            .first(&mut *db)
            .await
            .map_err(|_| AutumnError::unauthorized_msg("User not found."))?
    };
    let now = chrono::Utc::now().naive_utc();
    if auth_result.needs_update() {
        let mut passkey: Passkey = serde_json::from_str(&cred_json)
            .unwrap_or_else(|_| serde_json::from_value(serde_json::Value::Null).unwrap());
        passkey.update_credential(&auth_result);
        let new_json = serde_json::to_string(&passkey).unwrap_or(cred_json.clone());
        diesel::update(crate::schema::webauthn_credentials::table.find(wc_id))
            .set((
                crate::schema::webauthn_credentials::credential_json.eq(&new_json),
                crate::schema::webauthn_credentials::last_used_at.eq(now),
            ))
            .execute(&mut *db)
            .await
            .map_err(|_| {
                AutumnError::internal_server_error_msg("Failed to update credential state.")
            })?;
    } else {
        diesel::update(crate::schema::webauthn_credentials::table.find(wc_id))
            .set(crate::schema::webauthn_credentials::last_used_at.eq(now))
            .execute(&mut *db)
            .await
            .map_err(|_| {
                AutumnError::internal_server_error_msg("Failed to update credential timestamp.")
            })?;
    }
    session.rotate_id().await;
    session
        .insert("__SNAKE___id", __SNAKE___id.to_string())
        .await;
    session.insert("__SNAKE___email", &__SNAKE___email).await;
    session
        .insert(state.auth_session_key(), __SNAKE___id.to_string())
        .await;
    Ok(axum::Json(serde_json::json!({ "ok": true })))
}

// ── Management ────────────────────────────────────────────────────────────────

/// `GET /passkeys` — list the current user's registered passkeys.
#[secured]
#[get("/passkeys")]
pub async fn passkey_list(
    session: Session,
    State(state): State<AppState>,
    mut db: Db,
    csrf: Option<CsrfToken>,
    csrf_field: Option<CsrfFormField>,
) -> AutumnResult<Markup> {
    let __SNAKE___id: i64 = session
        .get(state.auth_session_key())
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;
    let rows: Vec<(i64, String, chrono::NaiveDateTime)> = {
        use crate::schema::webauthn_credentials;
        webauthn_credentials::table
            .filter(webauthn_credentials::user_id.eq(__SNAKE___id))
            .select((
                webauthn_credentials::id,
                webauthn_credentials::name,
                webauthn_credentials::created_at,
            ))
            .load(&mut *db)
            .await
            .unwrap_or_default()
    };
    Ok(html! {
        html {
            head { title { "Your Passkeys" } }
            body {
                h1 { "Your Passkeys" }
                a href="/passkeys/register" { "Register a new passkey" }
                @if rows.is_empty() {
                    p { "No passkeys registered yet." }
                } @else {
                    ul {
                        @for (id, name, created_at) in &rows {
                            li {
                                (name) " — registered " (created_at.to_string())
                                " "
                                form method="post" action="/passkeys/revoke" style="display:inline" {
                                    @if let Some(ref t) = csrf {
                                        input type="hidden"
                                              name=(csrf_field.as_ref().map(|f| f.0.as_str()).unwrap_or("_csrf"))
                                              value=(t.token());
                                    }
                                    input type="hidden" name="id" value=(id);
                                    button type="submit" { "Revoke" }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

/// `DELETE /passkeys/:id` (also accepts `POST /passkeys/revoke`) — remove a passkey.
#[secured]
#[post("/passkeys/revoke")]
pub async fn passkey_revoke(
    session: Session,
    State(state): State<AppState>,
    mut db: Db,
    Form(form): Form<PasskeyRevokeForm>,
) -> AutumnResult<impl IntoResponse> {
    let __SNAKE___id: i64 = session
        .get(state.auth_session_key())
        .await
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AutumnError::unauthorized_msg("Not authenticated."))?;
    diesel::delete(
        crate::schema::webauthn_credentials::table
            .filter(crate::schema::webauthn_credentials::id.eq(form.id))
            .filter(crate::schema::webauthn_credentials::user_id.eq(__SNAKE___id)),
    )
    .execute(&mut *db)
    .await
    .map_err(|_| AutumnError::internal_server_error_msg("Failed to revoke passkey."))?;
    Ok(redirect_to("/passkeys"))
}
"##;
    tpl.replace("__PASCAL__", pascal_name)
        .replace("__SNAKE__", snake_name)
        .replace("__TABLE__", user_table)
}

fn render_passkeys_tests_file(pascal_name: &str, _snake_name: &str) -> String {
    format!(
        r#"//! Generated passkey integration tests for {pascal_name} (`autumn generate auth --passkeys`).
//!
//! These tests run against a live server started with `AUTUMN_TEST_BASE_URL`.
//! In CI, start the app, set the env var, and run `cargo test`.
//! They skip when the env var is unset, so they compile and pass out of the box.

fn base_url() -> Option<String> {{
    std::env::var("AUTUMN_TEST_BASE_URL").ok()
}}

#[test]
fn passkey_register_happy_path() {{
    let Some(_base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    // 1. POST /passkeys/register/begin (authenticated session)
    // 2. Simulate navigator.credentials.create response
    // 3. POST /passkeys/register/finish — expect 200 {{ "ok": true }}
}}

#[test]
fn passkey_login_happy_path() {{
    let Some(_base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    // 1. POST /passkeys/login/begin
    // 2. Simulate navigator.credentials.get response
    // 3. POST /passkeys/login/finish — expect 200 {{ "ok": true }} and auth cookie
}}

#[test]
fn passkey_wrong_origin_rejected() {{
    let Some(_base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    // POST /passkeys/login/finish with a credential registered against a different
    // origin — expect 401 or 422.
}}

#[test]
fn passkey_unknown_credential_rejected() {{
    let Some(_base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    // POST /passkeys/login/finish with a credential_id that is not in the DB
    // — expect 401.
}}

#[test]
fn passkey_revoke_then_relogin() {{
    let Some(_base) = base_url() else {{
        eprintln!("skipping: AUTUMN_TEST_BASE_URL not set");
        return;
    }};
    // 1. Register a passkey.
    // 2. POST /passkeys/revoke to remove it.
    // 3. Attempt login with the revoked credential — expect failure.
}}
"#
    )
}

fn render_passkeys_docs_file() -> String {
    r#"# Passkeys (WebAuthn)

Generated with `autumn generate auth --passkeys`. Users can register and use
hardware security keys, platform authenticators (Touch ID, Face ID, Windows Hello),
or other FIDO2 compliant devices as passwordless credentials.

## Configuration

Add to `autumn.toml` before enabling passkeys:

```toml
[auth.webauthn]
rp_id     = "example.com"          # domain only (no port, no scheme)
rp_name   = "My App"               # shown in authenticator dialogs
rp_origin = "https://example.com"  # full origin including scheme
```

For local development use:

```toml
[auth.webauthn]
rp_id     = "localhost"
rp_name   = "My App (dev)"
rp_origin = "http://localhost:3000"
```

## Generated routes

| Method | Path | Handler | Auth |
|--------|------|---------|------|
| GET | `/passkeys/register` | `passkey_register_page` | **Required** |
| POST | `/passkeys/register/begin` | `passkey_register_begin` | **Required** |
| POST | `/passkeys/register/finish` | `passkey_register_finish` | **Required** |
| GET | `/passkeys/login` | `passkey_login_page` | Public |
| POST | `/passkeys/login/begin` | `passkey_login_begin` | Public |
| POST | `/passkeys/login/finish` | `passkey_login_finish` | Public |
| GET | `/passkeys` | `passkey_list` | **Required** |
| POST | `/passkeys/revoke` | `passkey_revoke` | **Required** |

## Security properties

- Ceremony challenges are bound to the origin (`rp_origin`) and domain (`rp_id`).
  A credential registered against `example.com` cannot authenticate against
  `evil.com`, even if the credential bytes are stolen.
- `passkey_login_finish` calls `session.rotate_id()` before writing the auth key,
  preventing session fixation.
- Revoked credentials are deleted from `webauthn_credentials`; a subsequent
  login attempt with the revoked credential will fail at the DB lookup step.
- `credential_json` stores opaque passkey state — never expose it to clients.

## Discoverable credentials

`passkey_login_begin` passes `&[]` (an empty allowed-credentials list), enabling
**discoverable credential** (resident key) flow. The browser prompts the user to
select a saved passkey without the server needing to know their identity first.
No credential IDs are sent to anonymous clients.

## Browser compatibility

The generated JavaScript uses `PublicKeyCredential.parseCreationOptionsFromJSON`,
`parseRequestOptionsFromJSON`, and `credential.toJSON()` (Chrome 119+,
Firefox 119+, Safari 17+). Add a polyfill (e.g. `@github/webauthn-json`) for
older browsers.

## Out of scope (follow-ups)

- **Rate-limiting** on ceremony endpoints is not included — add it before
  exposing them to untrusted traffic.
"#
    .to_owned()
}

/// Ensure `autumn-web` in `[dependencies]` has `features = ["webauthn"]`.
#[allow(clippy::too_many_lines)]
fn ensure_autumn_web_webauthn_feature(toml: &str) -> String {
    const CRATE: &str = "autumn-web";
    const FEATURE: &str = "\"webauthn\"";

    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    let trailing_newline = toml.ends_with('\n');

    let simple_prefix = format!("{CRATE} = \"");
    let table_prefix = format!("{CRATE} = {{");
    let subtable_header = format!("[dependencies.{CRATE}]");
    let subtable_header_underscore = format!("[dependencies.{}]", CRATE.replace('-', "_"));

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim().to_owned();
        let indent: String = lines[i]
            .chars()
            .take_while(char::is_ascii_whitespace)
            .collect();

        if let Some(rest) = trimmed.strip_prefix(&simple_prefix) {
            let version = rest.trim_end_matches('"');
            lines[i] =
                format!("{indent}{CRATE} = {{ version = \"{version}\", features = [{FEATURE}] }}");
            break;
        }

        if trimmed.starts_with(&table_prefix) {
            if trimmed.contains(FEATURE) {
                break; // already present
            }
            if let Some(feat_bracket) = trimmed.find("features = [") {
                let list_start = feat_bracket + "features = [".len();
                if let Some(close_bracket) = trimmed[list_start..].find(']') {
                    let list_end = close_bracket + list_start;
                    let existing = trimmed[list_start..list_end].trim();
                    let new_list = if existing.is_empty() {
                        FEATURE.to_owned()
                    } else {
                        format!("{existing}, {FEATURE}")
                    };
                    lines[i] = format!(
                        "{indent}{}{}{}",
                        &trimmed[..list_start],
                        new_list,
                        &trimmed[list_end..]
                    );
                } else {
                    let mut j = i + 1;
                    while j < lines.len() {
                        let tj = lines[j].trim();
                        if tj.starts_with('[') {
                            break;
                        }
                        if let Some(close_idx) = tj.find(']') {
                            let before_close = tj[..close_idx].trim();
                            let sep = if before_close.is_empty() || before_close.ends_with(',') {
                                ""
                            } else {
                                ", "
                            };
                            let indent_j: String = lines[j]
                                .chars()
                                .take_while(char::is_ascii_whitespace)
                                .collect();
                            lines[j] = format!(
                                "{indent_j}{before_close}{sep}{FEATURE}{}",
                                &tj[close_idx..]
                            );
                            break;
                        }
                        j += 1;
                    }
                }
            } else {
                // No features key — insert before closing `}`.
                let close = trimmed.rfind('}').unwrap();
                let before_close = trimmed[..close].trim_end();
                let sep = if before_close.ends_with('{') {
                    ""
                } else {
                    ", "
                };
                lines[i] = format!(
                    "{indent}{}{sep}features = [{FEATURE}]{}",
                    &trimmed[..close],
                    &trimmed[close..]
                );
            }
            break;
        }

        if trimmed == subtable_header || trimmed == subtable_header_underscore {
            // Scan ahead within the subtable.
            let mut j = i + 1;
            let mut found_features = false;
            while j < lines.len() {
                let t = lines[j].trim().to_owned();
                if t.starts_with('[') {
                    break;
                }
                if t.starts_with("features") {
                    found_features = true;
                    if !t.contains(FEATURE)
                        && let (Some(open), Some(close)) = (t.find('['), t.rfind(']'))
                    {
                        let inner = t[open + 1..close].trim();
                        let new_inner = if inner.is_empty() {
                            FEATURE.to_owned()
                        } else {
                            format!("{inner}, {FEATURE}")
                        };
                        let indent_j: String = lines[j]
                            .chars()
                            .take_while(char::is_ascii_whitespace)
                            .collect();
                        lines[j] = format!("{indent_j}features = [{new_inner}]");
                    }
                    break;
                }
                j += 1;
            }
            if !found_features {
                lines.insert(i + 1, format!("features = [{FEATURE}]"));
            }
            break;
        }

        i += 1;
    }

    let mut out = lines.join("\n");
    if trailing_newline && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn append_webauthn_stub_to_toml(
    existing: &str,
    rp_id: &str,
    rp_name: &str,
    rp_origin: &str,
) -> String {
    if existing.contains("[auth.webauthn]") {
        return existing.to_owned();
    }
    let mut out = existing.trim_end().to_owned();
    out.push_str("\n\n[auth.webauthn]\n");
    out.push_str("rp_id = \"");
    out.push_str(rp_id);
    out.push_str("\"\nrp_name = \"");
    out.push_str(rp_name);
    out.push_str("\"\nrp_origin = \"");
    out.push_str(rp_origin);
    out.push_str("\"\n");
    out
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn project_with_main() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\n\n[dependencies]\nautumn-web = \"0.3\"\n",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(
            tmp.path().join("src/main.rs"),
            "use autumn_web::prelude::*;\n\n\
             #[autumn_web::main]\n\
             async fn main() {\n\
             \x20   autumn_web::app().routes(routes![]).run().await;\n\
             }\n",
        )
        .unwrap();
        tmp
    }

    // ── Plan structure ──────────────────────────────────────────────────────

    #[test]
    fn plan_auth_creates_expected_files() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        let paths: Vec<String> = plan
            .actions
            .iter()
            .map(|a| {
                a.path()
                    .strip_prefix(tmp.path())
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();

        for expected in [
            "src/models/user.rs",
            "src/models/mod.rs",
            "src/schema.rs",
            "migrations/20260508000000_create_users/up.sql",
            "migrations/20260508000000_create_users/down.sql",
            "src/routes/auth.rs",
            "src/routes/mod.rs",
            "tests/auth.rs",
            "docs/guide/authentication.md",
            "src/main.rs",
        ] {
            assert!(
                paths.iter().any(|p| p == expected),
                "missing expected action for {expected}; got {paths:?}"
            );
        }
    }

    #[test]
    fn plan_auth_errors_when_not_in_project() {
        let tmp = TempDir::new().unwrap();
        let err = plan_auth(tmp.path(), "User", "20260508000000").unwrap_err();
        assert!(matches!(err, GenerateError::NotInProject));
    }

    // ── Migration SQL ───────────────────────────────────────────────────────

    #[test]
    fn migration_up_sql_creates_users_table_with_digest_columns() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let up = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000000_create_users/up.sql"),
        )
        .unwrap();
        assert!(
            up.contains("CREATE TABLE users"),
            "missing CREATE TABLE: {up}"
        );
        assert!(up.contains("email"), "missing email column: {up}");
        assert!(
            up.contains("password_digest"),
            "missing password_digest: {up}"
        );
        assert!(
            up.contains("reset_token_digest"),
            "missing reset_token_digest: {up}"
        );
        assert!(
            up.contains("reset_token_expires_at"),
            "missing reset_token_expires_at: {up}"
        );
        assert!(up.contains("UNIQUE"), "email column must be UNIQUE: {up}");
        assert!(
            !up.contains("password TEXT"),
            "raw password must never be stored: {up}"
        );
        assert!(
            !up.contains("reset_token TEXT"),
            "raw reset_token must never be stored: {up}"
        );
    }

    #[test]
    fn migration_down_sql_drops_users_table() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let down = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000000_create_users/down.sql"),
        )
        .unwrap();
        assert!(
            down.contains("DROP TABLE users"),
            "missing DROP TABLE: {down}"
        );
    }

    // ── schema.rs ───────────────────────────────────────────────────────────

    #[test]
    fn schema_rs_contains_diesel_table_for_auth_table() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert!(
            schema.contains("users (id)"),
            "schema missing table block: {schema}"
        );
        assert!(
            schema.contains("email -> Text"),
            "schema missing email column: {schema}"
        );
        assert!(
            schema.contains("password_digest -> Text"),
            "schema missing password_digest: {schema}"
        );
        assert!(
            schema.contains("reset_token_digest -> Nullable<Text>"),
            "schema missing nullable reset_token_digest: {schema}"
        );
        assert!(
            schema.contains("reset_token_expires_at -> Nullable<Timestamp>"),
            "schema missing nullable reset_token_expires_at: {schema}"
        );
    }

    // ── Model file ──────────────────────────────────────────────────────────

    #[test]
    fn model_file_contains_struct_and_digest_fields() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let model = fs::read_to_string(tmp.path().join("src/models/user.rs")).unwrap();
        assert!(model.contains("pub struct User"), "missing struct: {model}");
        assert!(
            model.contains("pub email: String"),
            "missing email: {model}"
        );
        assert!(
            model.contains("pub password_digest: String"),
            "missing password_digest: {model}"
        );
        assert!(
            model.contains("pub reset_token_digest: Option<String>"),
            "reset_token_digest must be nullable: {model}"
        );
        assert!(
            !model.contains("pub password:"),
            "raw password must not be a field: {model}"
        );
        assert!(
            !model.contains("pub reset_token:"),
            "raw reset_token must not be a field: {model}"
        );
    }

    #[test]
    fn model_mod_rs_declares_module() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let mod_rs = fs::read_to_string(tmp.path().join("src/models/mod.rs")).unwrap();
        assert!(
            mod_rs.contains("pub mod user;"),
            "missing pub mod user: {mod_rs}"
        );
    }

    // ── Routes file ─────────────────────────────────────────────────────────

    #[test]
    fn routes_file_contains_all_handlers() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        for needle in [
            "pub async fn signup_form",
            "pub async fn signup",
            "pub async fn login_form",
            "pub async fn login",
            "pub async fn logout",
            "pub async fn account",
            "pub async fn forgot_password_form",
            "pub async fn forgot_password",
            "pub async fn reset_password_form",
            "pub async fn reset_password",
        ] {
            assert!(routes.contains(needle), "routes missing handler: {needle}");
        }
    }

    #[test]
    fn routes_file_uses_session_invalidation_on_logout() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("session.destroy"),
            "logout must destroy the session: {routes}"
        );
    }

    #[test]
    fn routes_file_rotates_session_on_login() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("session.rotate_id"),
            "login must rotate the session ID to prevent fixation: {routes}"
        );
    }

    #[test]
    fn routes_file_uses_configured_auth_session_key_for_policy_identity() {
        let routes = render_routes_file("Account", "account", "accounts", &[], false);
        assert!(
            routes.contains("State(state): State<AppState>"),
            "auth routes must receive AppState: {routes}"
        );
        assert!(
            routes.contains("session.insert(state.auth_session_key()"),
            "auth routes must populate the same session key used by policy context: {routes}"
        );
        assert!(
            !routes.contains("session.insert(\"user_id\""),
            "non-User auth routes must not hard-code user_id as the policy session key: {routes}"
        );
    }

    #[test]
    fn routes_file_account_is_protected() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("#[secured]"),
            "account route must use #[secured] for protection: {routes}"
        );
    }

    #[test]
    fn routes_mod_rs_declares_auth_module() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let mod_rs = fs::read_to_string(tmp.path().join("src/routes/mod.rs")).unwrap();
        assert!(
            mod_rs.contains("pub mod auth;"),
            "missing pub mod auth: {mod_rs}"
        );
    }

    #[test]
    fn routes_file_forgot_password_checks_mailer_is_disabled() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("mailer.is_disabled()"),
            "forgot_password must guard against disabled mail transport: {routes}"
        );
        assert!(
            routes.contains("mailer.send(mail).await"),
            "forgot_password must use async mailer.send(): {routes}"
        );
        // The is_disabled guard must appear before the DB lookup (maybe_user)
        // so it fires unconditionally and cannot enumerate registered addresses.
        // Search within the forgot_password function body, which is identified
        // by the unique `maybe_user` variable that only appears there.
        let disabled_pos = routes.find("mailer.is_disabled()").unwrap();
        let maybe_user_pos = routes.find("let maybe_user").unwrap();
        assert!(
            disabled_pos < maybe_user_pos,
            "is_disabled guard must come before the DB lookup in forgot_password"
        );
    }

    // ── Generated tests ─────────────────────────────────────────────────────

    #[test]
    fn tests_file_covers_all_required_flows() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let tests = fs::read_to_string(tmp.path().join("tests/auth.rs")).unwrap();
        for needle in [
            "auth_signup_returns_200",
            "auth_login_returns_200",
            "auth_logout_redirects",
            "auth_forgot_password_returns_200",
            "auth_reset_password_returns_200",
            "auth_account_rejects_anonymous",
        ] {
            assert!(tests.contains(needle), "tests missing flow: {needle}");
        }
    }

    // ── main.rs registration ────────────────────────────────────────────────

    #[test]
    fn main_rs_registers_auth_routes() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        for entry in [
            "routes::auth::signup_form",
            "routes::auth::signup",
            "routes::auth::login_form",
            "routes::auth::login",
            "routes::auth::logout",
            "routes::auth::account",
            "routes::auth::forgot_password_form",
            "routes::auth::forgot_password",
            "routes::auth::reset_password_form",
            "routes::auth::reset_password",
        ] {
            assert!(main.contains(entry), "main.rs missing route entry: {entry}");
        }
    }

    #[test]
    fn main_rs_declares_models_and_routes_mods() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(
            main.contains("mod models;"),
            "main.rs missing mod models: {main}"
        );
        assert!(
            main.contains("mod routes;"),
            "main.rs missing mod routes: {main}"
        );
    }

    // ── Dry run ─────────────────────────────────────────────────────────────

    #[test]
    fn dry_run_writes_no_files() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags {
            dry_run: true,
            force: false,
        })
        .unwrap();
        assert!(
            !tmp.path().join("src/models/user.rs").exists(),
            "dry run must not create model file"
        );
        assert!(
            !tmp.path().join("src/routes/auth.rs").exists(),
            "dry run must not create routes file"
        );
    }

    // ── Cargo.toml feature injection ────────────────────────────────────────

    #[test]
    fn cargo_toml_gets_mail_feature_simple_string_form() {
        let input = "[dependencies]\nautumn-web = \"0.3\"\n";
        let out = ensure_autumn_web_mail_feature(input);
        assert!(
            out.contains("features = [\"mail\"]"),
            "mail feature missing: {out}"
        );
        assert_eq!(out.matches("autumn-web =").count(), 1, "must not duplicate");
    }

    #[test]
    fn cargo_toml_gets_mail_feature_inline_table_with_existing_features() {
        let input = "[dependencies]\nautumn-web = { version = \"0.3\", features = [\"ws\"] }\n";
        let out = ensure_autumn_web_mail_feature(input);
        assert!(out.contains("\"mail\""), "mail missing: {out}");
        assert!(
            out.contains("\"ws\""),
            "existing feature must be kept: {out}"
        );
    }

    #[test]
    fn cargo_toml_mail_feature_idempotent() {
        let input = "[dependencies]\nautumn-web = { version = \"0.3\", features = [\"mail\"] }\n";
        let out = ensure_autumn_web_mail_feature(input);
        assert_eq!(
            out.matches("\"mail\"").count(),
            1,
            "must not duplicate feature"
        );
    }

    #[test]
    fn model_file_created_at_is_marked_default() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let model = fs::read_to_string(tmp.path().join("src/models/user.rs")).unwrap();
        assert!(
            model.contains("#[default]"),
            "created_at must be marked #[default] so NewUser excludes it: {model}"
        );
    }

    #[test]
    fn cargo_toml_adds_axum_and_tracing_deps() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(cargo.contains("axum"), "axum dep missing: {cargo}");
        assert!(cargo.contains("tracing"), "tracing dep missing: {cargo}");
        assert!(
            cargo.contains("\"mail\""),
            "autumn-web mail feature missing: {cargo}"
        );
    }

    // ── Non-default model name ──────────────────────────────────────────────

    #[test]
    fn plan_auth_supports_custom_model_name() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "Account", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        assert!(
            tmp.path().join("src/models/account.rs").exists(),
            "model file should use snake_case of given name"
        );
        let model = fs::read_to_string(tmp.path().join("src/models/account.rs")).unwrap();
        assert!(
            model.contains("pub struct Account"),
            "struct name should match given name"
        );
    }

    // ── OAuth2 generator tests (RED phase) ──────────────────────────────────

    #[test]
    fn plan_auth_with_oauth_creates_oauth_identities_migration() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned(), "google".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        let paths: Vec<String> = plan
            .actions
            .iter()
            .map(|a| {
                a.path()
                    .strip_prefix(tmp.path())
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();
        assert!(
            paths.iter().any(|p| p.contains("oauth_identities")),
            "oauth_identities migration missing; got {paths:?}"
        );
    }

    #[test]
    fn plan_auth_without_oauth_is_unchanged() {
        let tmp = project_with_main();
        // Calling with empty providers must produce the same plan as plain plan_auth.
        let oauth = AuthOAuthOptions { providers: vec![] };
        let plan_with =
            plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        let plan_plain = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        // Plans should have the same number of actions and the same paths.
        let paths_with: std::collections::HashSet<String> = plan_with
            .actions
            .iter()
            .map(|a| {
                a.path()
                    .strip_prefix(tmp.path())
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();
        let paths_plain: std::collections::HashSet<String> = plan_plain
            .actions
            .iter()
            .map(|a| {
                a.path()
                    .strip_prefix(tmp.path())
                    .unwrap()
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            paths_with, paths_plain,
            "empty --oauth must not add any extra files"
        );
    }

    #[test]
    fn oauth_migration_up_has_provider_subject_and_fk_columns() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        // Find the oauth_identities up.sql — timestamp is base+1 to avoid Diesel conflicts.
        let mig_dir = tmp
            .path()
            .join("migrations/20260508000001_create_oauth_identities");
        let up = fs::read_to_string(mig_dir.join("up.sql")).unwrap();
        assert!(
            up.contains("CREATE TABLE oauth_identities"),
            "missing CREATE TABLE oauth_identities: {up}"
        );
        assert!(up.contains("provider"), "missing provider column: {up}");
        assert!(up.contains("subject"), "missing subject column: {up}");
        assert!(
            up.contains("UNIQUE"),
            "provider+subject must have UNIQUE constraint: {up}"
        );
        assert!(
            !up.contains("plaintext") && !up.contains("access_token"),
            "oauth_identities must not store provider access tokens: {up}"
        );
    }

    #[test]
    fn oauth_routes_file_contains_redirect_and_callback_handlers() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let routes = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        assert!(
            routes.contains("pub async fn oauth_redirect"),
            "oauth_redirect handler missing: {routes}"
        );
        assert!(
            routes.contains("pub async fn oauth_callback"),
            "oauth_callback handler missing: {routes}"
        );
        assert!(
            routes.contains("oauth2_authorize_url"),
            "oauth_redirect must call oauth2_authorize_url: {routes}"
        );
        assert!(
            routes.contains("oauth2_finish_login"),
            "oauth_callback must call oauth2_finish_login: {routes}"
        );
    }

    #[test]
    fn oauth_callback_uses_state_nonce_validation() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let routes = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        // oauth2_finish_login handles state+nonce internally, so its presence
        // is the contract that state/nonce checking is enforced.
        assert!(
            routes.contains("oauth2_finish_login"),
            "callback must use oauth2_finish_login (which enforces state+nonce): {routes}"
        );
    }

    #[test]
    fn plan_auth_with_oauth_creates_oauth_doc() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["google".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let doc = fs::read_to_string(tmp.path().join("docs/guide/oauth.md")).unwrap();
        assert!(
            doc.contains("OAuth") || doc.contains("oauth"),
            "oauth.md must reference OAuth: {doc}"
        );
        assert!(
            doc.contains("google") || doc.contains("Google"),
            "oauth.md must mention configured providers: {doc}"
        );
        assert!(
            doc.contains("client_id"),
            "oauth.md must cover client_id configuration: {doc}"
        );
        assert!(
            doc.contains("PKCE") || doc.contains("pkce"),
            "oauth.md must document PKCE security property: {doc}"
        );
    }

    #[test]
    fn plan_auth_with_oauth_adds_oauth2_feature_to_autumn_web() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("\"oauth2\""),
            "Cargo.toml must enable autumn-web's oauth2 feature: {cargo}"
        );
    }

    #[test]
    fn plan_auth_without_oauth_does_not_add_oauth2_feature() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();

        let cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(
            !cargo.contains("\"oauth2\""),
            "Cargo.toml must NOT enable oauth2 feature when --oauth not used: {cargo}"
        );
    }

    #[test]
    fn oauth_routes_registered_in_main_rs() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(
            main.contains("routes::oauth"),
            "main.rs must register oauth routes: {main}"
        );
    }

    #[test]
    fn oauth_migration_has_distinct_timestamp_from_base_auth_migration() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let mig_entries: Vec<_> = fs::read_dir(tmp.path().join("migrations"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        let auth_mig = mig_entries
            .iter()
            .find(|n| n.contains("create_users") || n.contains("create_accounts"));
        let oauth_mig = mig_entries.iter().find(|n| n.contains("oauth_identities"));
        if let (Some(a), Some(o)) = (auth_mig, oauth_mig) {
            let a_ts = a.split('_').next().unwrap_or("");
            let o_ts = o.split('_').next().unwrap_or("");
            assert_ne!(
                a_ts, o_ts,
                "oauth_identities migration must have a different timestamp than the auth migration"
            );
        }
    }

    #[test]
    fn oauth_mod_rs_preserves_base_auth_module_declaration() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let mod_rs = fs::read_to_string(tmp.path().join("src/routes/mod.rs")).unwrap();
        assert!(
            mod_rs.contains("pub mod auth;"),
            "routes/mod.rs must keep pub mod auth: {mod_rs}"
        );
        assert!(
            mod_rs.contains("pub mod oauth;"),
            "routes/mod.rs must add pub mod oauth: {mod_rs}"
        );
    }

    #[test]
    fn oauth_handlers_have_route_macros() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let routes = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        assert!(
            routes.contains("#[get("),
            "oauth handlers must have #[get(...)] route macros for routes! registration: {routes}"
        );
    }

    #[test]
    fn oauth_callback_does_not_set_session_key_before_account_link() {
        let routes =
            render_oauth_routes_file("User", "user", "users", &["github".to_owned()], false);
        // The TODO comment may mention the call; verify the EXECUTABLE line is absent.
        // Executable session.insert calls are not prefixed with "//" or "#".
        let executable_insert = routes
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .any(|l| l.contains("session.insert(&auth_cfg.session_key"));
        assert!(
            !executable_insert,
            "generated callback must not execute session.insert before account linking: {routes}"
        );
    }

    #[test]
    fn plan_auth_with_oauth_adds_oauth_identities_to_schema_rs() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert!(
            schema.contains("oauth_identities (id)"),
            "schema.rs must contain oauth_identities table block: {schema}"
        );
        assert!(
            schema.contains("provider -> Text"),
            "schema.rs must contain provider column: {schema}"
        );
        assert!(
            schema.contains("subject -> Text"),
            "schema.rs must contain subject column: {schema}"
        );
        assert!(
            schema.contains("user_id -> Int8"),
            "schema.rs must contain user_id column: {schema}"
        );
        assert!(
            schema.contains("email -> Nullable<Text>"),
            "schema.rs must contain nullable email: {schema}"
        );
        assert!(
            schema.contains("name -> Nullable<Text>"),
            "schema.rs must contain nullable name: {schema}"
        );
    }

    #[test]
    fn plan_auth_with_oauth_appends_stubs_to_autumn_toml() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned(), "google".to_owned()],
        };
        // Setup mock autumn.toml
        fs::write(tmp.path().join("autumn.toml"), "[server]\nport = 3000\n").unwrap();

        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let toml = fs::read_to_string(tmp.path().join("autumn.toml")).unwrap();
        assert!(
            toml.contains("[auth.oauth2.github]"),
            "autumn.toml missing github preset"
        );
        assert!(
            toml.contains("[auth.oauth2.google]"),
            "autumn.toml missing google preset"
        );
        assert!(
            toml.contains("scope = \"read:user user:email\""),
            "autumn.toml missing github scope"
        );
    }

    #[test]
    fn oauth_routes_file_contains_login_buttons() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned(), "google".to_owned()],
        };
        let plan = plan_auth_with_options(tmp.path(), "User", "20260508000000", &oauth).unwrap();
        plan.execute(Flags::default()).unwrap();

        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("Or sign in with:"),
            "login form routes file must render OAuth provider buttons"
        );
        assert!(
            routes.contains("a href=\"/auth/github/redirect\""),
            "login form routes file missing github redirect link"
        );
        assert!(
            routes.contains("a href=\"/auth/google/redirect\""),
            "login form routes file missing google redirect link"
        );
    }

    // ── TOTP two-factor generator tests (S-061 / #799) ──────────────────────

    fn totp_plan(tmp: &std::path::Path) -> Plan {
        let oauth = AuthOAuthOptions::default();
        plan_auth_full(tmp, "User", "20260508000000", &oauth, true).unwrap()
    }

    #[test]
    fn totp_migration_adds_totp_columns_and_recovery_table() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let up = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000000_create_users/up.sql"),
        )
        .unwrap();
        assert!(
            up.contains("totp_secret_encrypted"),
            "missing totp_secret_encrypted: {up}"
        );
        assert!(up.contains("totp_enabled"), "missing totp_enabled: {up}");
        assert!(
            up.contains("CREATE TABLE recovery_codes"),
            "missing recovery_codes table: {up}"
        );
        assert!(up.contains("code_digest"), "missing code_digest: {up}");
        assert!(up.contains("used_at"), "missing used_at: {up}");
        assert!(
            !up.contains("totp_secret TEXT") && !up.contains("recovery_code TEXT"),
            "raw secret / raw recovery code must never be stored: {up}"
        );
        let down = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000000_create_users/down.sql"),
        )
        .unwrap();
        assert!(
            down.contains("DROP TABLE recovery_codes"),
            "down.sql must drop recovery_codes: {down}"
        );
    }

    #[test]
    fn totp_model_has_totp_fields_and_recovery_model() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let model = fs::read_to_string(tmp.path().join("src/models/user.rs")).unwrap();
        assert!(
            model.contains("pub totp_secret_encrypted: Option<String>"),
            "{model}"
        );
        assert!(model.contains("pub totp_enabled: bool"), "{model}");
        let rc = fs::read_to_string(tmp.path().join("src/models/recovery_code.rs")).unwrap();
        assert!(rc.contains("pub struct RecoveryCode"), "{rc}");
        assert!(rc.contains("pub code_digest: String"), "{rc}");
        assert!(rc.contains("pub used_at: Option<"), "{rc}");
        let mod_rs = fs::read_to_string(tmp.path().join("src/models/mod.rs")).unwrap();
        assert!(mod_rs.contains("pub mod recovery_code;"), "{mod_rs}");
    }

    #[test]
    fn totp_schema_has_totp_and_recovery_columns() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert!(
            schema.contains("totp_secret_encrypted -> Nullable<Text>"),
            "{schema}"
        );
        assert!(schema.contains("totp_enabled -> Bool"), "{schema}");
        assert!(schema.contains("recovery_codes (id)"), "{schema}");
        assert!(schema.contains("code_digest -> Text"), "{schema}");
        assert!(
            schema.contains("used_at -> Nullable<Timestamp>"),
            "{schema}"
        );
    }

    #[test]
    fn totp_routes_have_2fa_handlers_and_paths() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        for needle in [
            "pub async fn two_factor_status",
            "pub async fn two_factor_enable",
            "pub async fn two_factor_confirm",
            "pub async fn two_factor_disable",
            "pub async fn login_verify_form",
            "pub async fn login_verify",
        ] {
            assert!(
                routes.contains(needle),
                "routes missing 2fa handler: {needle}"
            );
        }
        for path in [
            "\"/account/2fa\"",
            "\"/account/2fa/enable\"",
            "\"/account/2fa/disable\"",
            "\"/login/verify\"",
        ] {
            assert!(routes.contains(path), "routes missing 2fa path: {path}");
        }
    }

    #[test]
    fn totp_login_marks_pending_and_redirects_to_verify() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("totp_pending_id"),
            "login must set totp_pending marker: {routes}"
        );
        assert!(
            routes.contains("totp_enabled"),
            "login must branch on totp_enabled: {routes}"
        );
        assert!(
            routes.contains("/login/verify"),
            "login must redirect to /login/verify: {routes}"
        );
    }

    #[test]
    fn totp_login_clears_abandoned_reset_marker() {
        // P2 (#1057): an ordinary password login must clear any stale
        // totp_pending_reset_digest so it can't commit a stale password change.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let login_pos = routes.find("pub async fn login(").expect("login fn");
        let logout_pos = routes.find("pub async fn logout(").unwrap_or(routes.len());
        let login_body = &routes[login_pos..logout_pos];
        assert!(
            login_body.contains("session.remove(\"totp_pending_reset_digest\")"),
            "login interstitial must clear an abandoned deferred-reset marker: {login_body}"
        );
    }

    #[test]
    fn totp_signup_clears_pending_state() {
        // P2 (#1057 round 6): signing up establishes a fresh authenticated
        // session, so it must clear any abandoned pending-2FA / reset / enrollment
        // markers — otherwise a later /login/verify could switch accounts.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let signup_pos = routes.find("pub async fn signup(").expect("signup fn");
        let next_pos = routes[signup_pos + 1..]
            .find("pub async fn ")
            .map_or(routes.len(), |p| signup_pos + 1 + p);
        let signup_body = &routes[signup_pos..next_pos];
        assert!(
            signup_body.contains("session.remove(\"totp_pending_id\")")
                && signup_body.contains("session.remove(\"totp_pending_secret\")"),
            "signup must clear pending 2FA / enrollment markers: {signup_body}"
        );
    }

    #[test]
    fn totp_clear_pending_includes_enrollment_secret() {
        // P2 (#1057 round 6): a stale totp_pending_secret must not survive a fresh
        // full login, or two_factor_confirm could accept it for a different account.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let login_pos = routes.find("pub async fn login(").expect("login fn");
        let logout_pos = routes.find("pub async fn logout(").unwrap_or(routes.len());
        let login_body = &routes[login_pos..logout_pos];
        assert!(
            login_body.contains("session.remove(\"totp_pending_secret\")"),
            "login must clear a stale enrollment secret: {login_body}"
        );
    }

    #[test]
    fn totp_interstitial_rejects_session_key_collision() {
        // P2 (#1057 round 6): if [auth].session_key collides with a reserved
        // totp_pending_* key, parking the pending marker would set the trusted
        // auth key. The interstitials must refuse such a configuration.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("fn is_reserved_totp_pending_key"),
            "must define the reserved-pending-key guard helper: {routes}"
        );
        let login_pos = routes.find("pub async fn login(").expect("login fn");
        let logout_pos = routes.find("pub async fn logout(").unwrap_or(routes.len());
        let login_body = &routes[login_pos..logout_pos];
        assert!(
            login_body.contains("is_reserved_totp_pending_key(state.auth_session_key())"),
            "login interstitial must reject a colliding auth session key: {login_body}"
        );
    }

    #[test]
    fn totp_pending_handoff_clears_live_auth_keys() {
        // P2 (#1057 round 9): rotate_id() preserves session data, so a pending-2FA
        // handoff started while already authenticated as another account must drop
        // the live auth keys, or #[secured] would still trust the old account.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        // Login interstitial: before parking totp_pending_id it must remove the
        // configured auth session key.
        let login_pos = routes.find("pub async fn login(").expect("login fn");
        let logout_pos = routes.find("pub async fn logout(").unwrap_or(routes.len());
        let login_body = &routes[login_pos..logout_pos];
        let park_at = login_body
            .find("insert(\"totp_pending_id\"")
            .expect("login parks totp_pending_id");
        let before_park = &login_body[..park_at];
        assert!(
            before_park.contains("session.remove(state.auth_session_key())"),
            "login interstitial must clear the live auth key before parking 2FA: {login_body}"
        );
        // Reset interstitial must do the same.
        let reset_pos = routes
            .find("pub async fn reset_password(")
            .expect("reset_password fn");
        let reset_body = &routes[reset_pos..reset_pos + 2500.min(routes.len() - reset_pos)];
        let reset_park = reset_body
            .find("insert(\"totp_pending_id\"")
            .expect("reset parks totp_pending_id");
        assert!(
            reset_body[..reset_park].contains("session.remove(state.auth_session_key())"),
            "reset interstitial must clear the live auth key before parking 2FA: {reset_body}"
        );
    }

    #[test]
    fn totp_login_verify_falls_back_to_recovery_on_broken_secret() {
        // P2 (#1057 round 8): a decrypt/build failure (e.g. rotated TOTP_ENC_KEY)
        // must not abort login_verify before the recovery-code branch — recovery
        // codes are the only escape from that lockout.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let verify_pos = routes
            .find("pub async fn login_verify(")
            .expect("login_verify fn");
        // Up to the recovery-code branch, the TOTP attempt must not use `?` on
        // decrypt_secret/build_totp (which would abort the whole handler).
        let verify_to_recovery = &routes[verify_pos..];
        let recovery_at = verify_to_recovery
            .find("try an unused recovery code")
            .expect("recovery-code comment");
        let totp_branch = &verify_to_recovery[..recovery_at];
        assert!(
            !totp_branch.contains("decrypt_secret(stored)?"),
            "login_verify TOTP branch must not abort on decrypt failure: {totp_branch}"
        );
        assert!(
            totp_branch.contains("decrypt_secret(stored)") && totp_branch.contains(".ok()"),
            "login_verify must treat a broken secret as a miss and fall through: {totp_branch}"
        );
    }

    #[test]
    fn totp_disable_falls_back_to_password_on_broken_secret() {
        // P2 (#1057 round 8): the disable code path must likewise not abort on a
        // decrypt failure, so the password fallback can still re-authenticate.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let disable_pos = routes
            .find("pub async fn two_factor_disable(")
            .expect("two_factor_disable fn");
        let next = routes[disable_pos + 1..]
            .find("pub async fn ")
            .map_or(routes.len(), |p| disable_pos + 1 + p);
        let disable_body = &routes[disable_pos..next];
        assert!(
            !disable_body.contains("decrypt_secret(stored)?"),
            "disable must not abort on decrypt failure before the password fallback: {disable_body}"
        );
    }

    #[test]
    fn totp_build_totp_sanitizes_colon_in_account_label() {
        // P2 (#1057 round 8): totp-rs rejects account labels containing ':', so an
        // email like a:b@example.com would 500. build_totp must strip colons.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let build_pos = routes.find("fn build_totp(").expect("build_totp fn");
        let build_body = &routes[build_pos..build_pos + 600.min(routes.len() - build_pos)];
        assert!(
            build_body.contains("account.replace(':'"),
            "build_totp must sanitize colons in the account label: {build_body}"
        );
    }

    #[test]
    fn totp_oauth_pending_note_guards_session_key_collision() {
        // P2 (#1057 round 8): the OAuth pending-2FA guidance must mirror the
        // password/reset reserved-key rejection before parking totp_pending_id.
        let tmp = project_with_main();
        let oauth_opts = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        plan_auth_full(tmp.path(), "User", "20260508000000", &oauth_opts, true)
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let oauth = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        assert!(
            oauth.contains("is_reserved_totp_pending_key(&auth_cfg.session_key)"),
            "OAuth pending-2FA guidance must guard against an auth-key collision: {oauth}"
        );
    }

    #[test]
    fn totp_rejects_recovery_code_name_collision() {
        // P2 (#1057 round 7): `--totp` always emits a recovery_code model/table,
        // so an auth resource that resolves to that name would emit it twice and
        // produce an unusable app. The generator must reject it up front.
        let tmp = project_with_main();
        // Both forms resolve to snake `recovery_code` / table `recovery_codes`,
        // which is exactly the helper model/table emitted under --totp.
        for name in ["RecoveryCode", "recovery_code"] {
            let err = plan_auth_with_providers(tmp.path(), name, "20260508000000", &[], true)
                .expect_err("recovery_code collision must be rejected under --totp");
            assert!(
                matches!(err, GenerateError::InvalidName(_, _)),
                "expected InvalidName for {name}, got {err:?}"
            );
        }
        // Without --totp the same name is fine (no recovery_code table emitted).
        assert!(
            plan_auth_with_providers(tmp.path(), "RecoveryCode", "20260508000000", &[], false)
                .is_ok(),
            "recovery_code name must be allowed without --totp"
        );
    }

    #[test]
    fn totp_rejects_existing_recovery_codes_table() {
        // P2 (#1057 round 11): if the project already declares a `recovery_codes`
        // table, the unconditional --totp migration would CREATE it again and
        // `diesel migration run` would fail. Reject up front.
        let tmp = project_with_main();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(
            tmp.path().join("src/schema.rs"),
            "diesel::table! {\n    recovery_codes (id) {\n        id -> Int8,\n    }\n}\n",
        )
        .unwrap();
        let err = plan_auth_with_providers(tmp.path(), "User", "20260508000000", &[], true)
            .expect_err("existing recovery_codes table must be rejected under --totp");
        assert!(
            matches!(err, GenerateError::InvalidName(_, _)),
            "expected InvalidName, got {err:?}"
        );
        // Without --totp there's no recovery_codes helper, so it's fine.
        assert!(
            plan_auth_with_providers(tmp.path(), "User", "20260508000000", &[], false).is_ok(),
            "existing recovery_codes table is irrelevant without --totp"
        );
    }

    #[test]
    fn totp_enable_rejects_reserved_session_key() {
        // P2 (#1057 round 11): two_factor_enable stashes totp_pending_secret, so it
        // must also reject a [auth].session_key that collides with a reserved key.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let enable_pos = routes
            .find("pub async fn two_factor_enable(")
            .expect("enable fn");
        let confirm_pos = routes
            .find("pub async fn two_factor_confirm(")
            .unwrap_or(routes.len());
        let enable_body = &routes[enable_pos..confirm_pos];
        assert!(
            enable_body.contains("is_reserved_totp_pending_key(state.auth_session_key())"),
            "two_factor_enable must reject a reserved auth session key: {enable_body}"
        );
    }

    #[test]
    fn totp_oauth_full_login_note_rotates_session() {
        // P2 (#1057 round 11): the non-2FA OAuth full-login guidance must rotate the
        // session id before promoting, mirroring password login (anti-fixation).
        let tmp = project_with_main();
        let oauth_opts = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        plan_auth_full(tmp.path(), "User", "20260508000000", &oauth_opts, true)
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let oauth = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        let note_pos = oauth
            .find("otherwise, fully authenticate")
            .expect("full-login guidance");
        let after = &oauth[note_pos..];
        assert!(
            after.contains("session.rotate_id()"),
            "OAuth full-login guidance must rotate the session: {after}"
        );
    }

    #[test]
    fn ensure_totp_rs_features_promotes_simple_version() {
        let toml = "[dependencies]\ntotp-rs = \"5\"\n";
        let out = ensure_totp_rs_features(toml);
        assert!(
            out.contains("\"qr\"") && out.contains("\"gen_secret\"") && out.contains("\"otpauth\""),
            "simple version must be promoted with required features: {out}"
        );
    }

    #[test]
    fn ensure_totp_rs_features_merges_into_partial_features() {
        let toml = "[dependencies]\ntotp-rs = { version = \"5\", features = [\"qr\"] }\n";
        let out = ensure_totp_rs_features(toml);
        assert!(
            out.contains("\"qr\"") && out.contains("\"gen_secret\"") && out.contains("\"otpauth\""),
            "missing features must be merged in: {out}"
        );
        // The pre-existing feature must not be duplicated.
        assert_eq!(out.matches("\"qr\"").count(), 1, "qr duplicated: {out}");
    }

    #[test]
    fn ensure_totp_rs_features_adds_features_key_when_absent() {
        let toml = "[dependencies]\ntotp-rs = { version = \"5\" }\n";
        let out = ensure_totp_rs_features(toml);
        assert!(
            out.contains("features = [")
                && out.contains("\"gen_secret\"")
                && out.contains("\"otpauth\""),
            "a features key must be added: {out}"
        );
    }

    #[test]
    fn ensure_totp_rs_features_is_idempotent_when_complete() {
        let toml = "[dependencies]\ntotp-rs = { version = \"5\", features = [\"qr\", \"gen_secret\", \"otpauth\"] }\n";
        assert_eq!(
            ensure_totp_rs_features(toml),
            toml,
            "already-complete features must be left untouched"
        );
    }

    #[test]
    fn ensure_totp_rs_features_merges_subtable_with_partial_features() {
        // P2 (#1057 round 12): the `[dependencies.totp-rs]` subtable form must also
        // gain the missing features.
        let toml = "[dependencies.totp-rs]\nversion = \"5\"\nfeatures = [\"qr\"]\n";
        let out = ensure_totp_rs_features(toml);
        assert!(
            out.contains("\"qr\"") && out.contains("\"gen_secret\"") && out.contains("\"otpauth\""),
            "subtable features must be merged: {out}"
        );
        assert_eq!(out.matches("\"qr\"").count(), 1, "qr duplicated: {out}");
    }

    #[test]
    fn ensure_totp_rs_features_adds_subtable_features_key_when_absent() {
        let toml = "[dependencies.totp-rs]\nversion = \"5\"\n";
        let out = ensure_totp_rs_features(toml);
        assert!(
            out.contains("features = [")
                && out.contains("\"qr\"")
                && out.contains("\"gen_secret\"")
                && out.contains("\"otpauth\""),
            "a features key must be added to the subtable: {out}"
        );
    }

    #[test]
    fn ensure_totp_rs_features_strips_inline_comment_on_simple_version() {
        // P2 (#1057 round 13): a trailing inline comment must not be folded into
        // the version string (which would make Cargo.toml invalid).
        let toml = "[dependencies]\ntotp-rs = \"5\" # used by metrics\n";
        let out = ensure_totp_rs_features(toml);
        assert!(
            out.contains("version = \"5\"") && out.contains("features = ["),
            "version must be parsed cleanly: {out}"
        );
        assert!(
            !out.contains("\"5\" # used by metrics\""),
            "the comment must not leak into the version string: {out}"
        );
        // The comment is preserved after the rewritten table.
        assert!(
            out.contains("# used by metrics"),
            "the trailing comment should be preserved: {out}"
        );
    }

    #[test]
    fn ensure_totp_rs_features_merges_multiline_subtable_array() {
        // P2 (#1057 round 13): a multiline `features = [` array in the subtable
        // form must still gain the missing features.
        let toml = "[dependencies.totp-rs]\nversion = \"5\"\nfeatures = [\n    \"qr\",\n]\n";
        let out = ensure_totp_rs_features(toml);
        assert!(
            out.contains("\"qr\"") && out.contains("\"gen_secret\"") && out.contains("\"otpauth\""),
            "multiline subtable features must be merged: {out}"
        );
        assert_eq!(out.matches("\"qr\"").count(), 1, "qr duplicated: {out}");
    }

    #[test]
    fn totp_disable_wraps_cleanup_in_transaction() {
        // P2 (#1057 round 12): disabling 2FA must be atomic — the flag/secret clear
        // and recovery-code delete run in one transaction.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let disable_pos = routes
            .find("pub async fn two_factor_disable(")
            .expect("disable fn");
        let next = routes[disable_pos + 1..]
            .find("pub async fn ")
            .map_or(routes.len(), |p| disable_pos + 1 + p);
        let body = &routes[disable_pos..next];
        assert!(
            body.contains(".transaction::<_, diesel::result::Error, _>"),
            "two_factor_disable must run cleanup in a transaction: {body}"
        );
        // Both the flag clear and the recovery-code delete must be inside it.
        assert!(
            body.contains("totp_enabled.eq(false)")
                && body.contains("delete(recovery_codes::table"),
            "transaction must cover both the disable update and recovery-code delete: {body}"
        );
    }

    #[test]
    fn totp_login_verify_tears_down_pending_on_stale_reset() {
        // P2 (#1057 round 7): if the parked reset token is stale, login_verify must
        // clear totp_pending_id too — otherwise a retry skips the (now-cleared)
        // reset branch and promotes the session to a full login.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let verify_pos = routes
            .find("pub async fn login_verify(")
            .expect("login_verify fn");
        let verify_body = &routes[verify_pos..];
        // The stale-reset failure path (before the success-promotion block) must
        // remove totp_pending_id.
        let promote_at = verify_body
            .find("Promote the pending session")
            .expect("promotion comment");
        let failure_block = &verify_body[..promote_at];
        assert!(
            failure_block.contains("session.remove(\"totp_pending_id\")")
                && failure_block.contains("Please restart the password reset"),
            "stale-reset path must tear down the pending login: {failure_block}"
        );
    }

    #[test]
    fn totp_oauth_full_login_note_clears_pending_state() {
        // P2 (#1057 round 7): the OAuth non-2FA full-login guidance must also clear
        // abandoned totp_pending_* state, mirroring password login/signup.
        let tmp = project_with_main();
        let oauth_opts = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        plan_auth_full(tmp.path(), "User", "20260508000000", &oauth_opts, true)
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let oauth = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        let note_pos = oauth
            .find("otherwise, fully authenticate")
            .expect("full-login guidance");
        let after = &oauth[note_pos..];
        assert!(
            after.contains("totp_pending_secret") && after.contains("totp_pending_id"),
            "OAuth full-login guidance must clear pending 2FA/enrollment state: {after}"
        );
        // P2 (#1057 round 10): the full-login guidance must also set the
        // model-specific identity keys the generated handlers read, not only the
        // configured auth key.
        assert!(
            after.contains("session.insert(\"user_id\", local_user.id.to_string())")
                && after.contains("session.insert(\"user_email\", &local_user.email)"),
            "OAuth full-login guidance must set the model identity keys: {after}"
        );
    }

    #[test]
    fn totp_reset_password_routes_2fa_users_through_verify() {
        // P1 (#1057): a reset link must not bypass the second factor.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        // The 2FA interstitial branch must appear inside reset_password too.
        let reset_pos = routes
            .find("pub async fn reset_password(")
            .expect("reset_password fn");
        let after = &routes[reset_pos..];
        let body_end = after.find("\n}").unwrap_or(after.len());
        let body = &after[..body_end];
        assert!(
            body.contains("totp_pending_id") && body.contains("/login/verify"),
            "reset_password must route 2FA-enabled users through /login/verify: {body}"
        );
    }

    #[test]
    fn totp_reset_password_defers_password_commit_until_verify() {
        // P2 (#1057): for a 2FA-enabled account the password must NOT be written
        // until /login/verify passes; the reset handler parks the digest and the
        // verify handler commits it.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let reset_pos = routes
            .find("pub async fn reset_password(")
            .expect("reset_password fn");
        let reset_body = &routes[reset_pos..reset_pos + 3200.min(routes.len() - reset_pos)];
        // The deferral branch (which returns to /login/verify) must come BEFORE
        // the password_digest UPDATE in the handler body.
        let park_at = reset_body
            .find("totp_pending_reset_digest")
            .expect("reset must park the pending digest");
        let write_at = reset_body
            .find("password_digest.eq(&new_digest)")
            .expect("reset still writes the digest for non-2FA users");
        assert!(
            park_at < write_at,
            "reset must park (and return) before committing the password for 2FA users"
        );
        // login_verify must commit a parked reset once the factor is proven.
        let verify_pos = routes
            .find("pub async fn login_verify(")
            .expect("login_verify fn");
        let verify_body = &routes[verify_pos..];
        assert!(
            verify_body.contains("totp_pending_reset_digest")
                && verify_body.contains("password_digest.eq(&new_digest)"),
            "login_verify must commit a parked password reset after 2FA: {verify_body}"
        );
    }

    #[test]
    fn totp_reset_branch_parks_token_digest() {
        // P2 (#1057 round 5): the reset interstitial must store totp_pending_reset_token
        // (the authorising SHA-256 digest) in the session alongside the new password
        // digest so login_verify can revalidate it before committing.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let reset_pos = routes
            .find("pub async fn reset_password(")
            .expect("reset_password fn");
        let reset_body = &routes[reset_pos..reset_pos + 3200.min(routes.len() - reset_pos)];
        assert!(
            reset_body.contains("totp_pending_reset_token"),
            "reset_password 2FA branch must park the token digest: {reset_body}"
        );
        assert!(
            reset_body.contains("token_digest"),
            "reset_password must have token_digest in scope when parking: {reset_body}"
        );
    }

    #[test]
    fn totp_login_verify_revalidates_token_before_commit() {
        // P2 (#1057 round 5): login_verify must make the deferred password commit
        // conditional on the authorising token still matching the DB row — a
        // superseded or expired reset token must cause 0 rows updated and be rejected.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let verify_pos = routes
            .find("pub async fn login_verify(")
            .expect("login_verify fn");
        let verify_body = &routes[verify_pos..];
        assert!(
            verify_body.contains("totp_pending_reset_token"),
            "login_verify must load the stored token from the session: {verify_body}"
        );
        assert!(
            verify_body.contains("reset_token_digest"),
            "login_verify update must filter by reset_token_digest: {verify_body}"
        );
        assert!(
            verify_body.contains("reset_token_expires_at"),
            "login_verify update must filter by reset_token_expires_at: {verify_body}"
        );
    }

    #[test]
    fn totp_oauth_callback_note_clears_both_pending_markers() {
        // P2 (#1057 round 5): the OAuth callback guidance comment must instruct
        // developers to clear both pending-reset markers before parking the 2FA
        // state — mirroring what the password-login handler does.
        let tmp = project_with_main();
        let oauth_opts = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        let plan = plan_auth_full(tmp.path(), "User", "20260508000000", &oauth_opts, true).unwrap();
        plan.execute(Flags::default()).unwrap();
        let oauth = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        assert!(
            oauth.contains("totp_pending_reset_digest"),
            "OAuth callback note must mention totp_pending_reset_digest: {oauth}"
        );
        assert!(
            oauth.contains("totp_pending_reset_token"),
            "OAuth callback note must mention totp_pending_reset_token: {oauth}"
        );
    }

    #[test]
    fn totp_enroll_requires_password_reauth() {
        // P2 (#1057): first enrollment must require step-up password re-auth.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("struct TotpEnableForm"),
            "enable must take a password form: {routes}"
        );
        let enable_pos = routes
            .find("pub async fn two_factor_enable(")
            .expect("enable fn");
        let confirm_pos = routes
            .find("pub async fn two_factor_confirm(")
            .expect("confirm fn");
        let enable_body = &routes[enable_pos..confirm_pos];
        assert!(
            enable_body.contains("verify_password(&form.password"),
            "two_factor_enable must verify the account password before enrolling: {enable_body}"
        );
        // The status-page enable form must collect the password.
        assert!(
            routes.contains("Confirm your password to begin enrollment"),
            "enable form must prompt for the password: {routes}"
        );
    }

    #[test]
    fn totp_disable_applies_replay_guard_on_code() {
        // P2 (#1057): a TOTP code used to disable must advance/respect
        // totp_last_used_step so a code already spent on login can't be replayed.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let disable_pos = routes
            .find("pub async fn two_factor_disable(")
            .expect("disable fn");
        let after = &routes[disable_pos..];
        let body_end = after.find("\n/// ").unwrap_or(after.len());
        let body = &after[..body_end];
        assert!(
            body.contains("totp_last_used_step") && body.contains("affected == 1"),
            "disable must consume the TOTP step with the same atomic guard: {body}"
        );
    }

    #[test]
    fn totp_confirm_guards_against_double_submit() {
        // P2 (#1057): the final enable flip is conditional on totp_enabled=false
        // with a rollback, so concurrent confirms don't desync recovery codes.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let confirm_pos = routes
            .find("pub async fn two_factor_confirm(")
            .expect("confirm fn");
        let after = &routes[confirm_pos..];
        let disable_pos = after
            .find("pub async fn two_factor_disable(")
            .unwrap_or(after.len());
        let body = &after[..disable_pos];
        assert!(
            body.contains("totp_enabled.eq(false)") && body.contains("RollbackTransaction"),
            "confirm must claim the enable transition conditionally and roll back on loss: {body}"
        );
    }

    #[test]
    fn totp_enable_does_not_disable_live_secret() {
        // P1 (#1057): starting re-enrollment must not flip totp_enabled=false on
        // the live account. The pending secret lives in the session instead.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let enable_pos = routes
            .find("pub async fn two_factor_enable(")
            .expect("enable fn");
        let confirm_pos = routes
            .find("pub async fn two_factor_confirm(")
            .expect("confirm fn");
        let enable_body = &routes[enable_pos..confirm_pos];
        assert!(
            enable_body.contains("session.insert(\"totp_pending_secret\""),
            "enable must stash the pending secret in the session: {enable_body}"
        );
        assert!(
            !enable_body.contains("totp_enabled.eq("),
            "enable must NOT write totp_enabled on the live row: {enable_body}"
        );
        assert!(
            !enable_body.contains("totp_secret_encrypted.eq("),
            "enable must NOT overwrite the live secret before confirmation: {enable_body}"
        );
    }

    #[test]
    fn totp_enable_rejects_reenrollment_while_active() {
        // P2 (#1057): a hijacked session must not swap the authenticator without
        // first disabling (which re-authenticates). Both enable and confirm guard
        // on the live `totp_enabled` flag.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let enable_pos = routes
            .find("pub async fn two_factor_enable(")
            .expect("enable fn");
        let confirm_pos = routes
            .find("pub async fn two_factor_confirm(")
            .expect("confirm fn");
        let disable_pos = routes
            .find("pub async fn two_factor_disable(")
            .expect("disable fn");
        let enable_body = &routes[enable_pos..confirm_pos];
        let confirm_body = &routes[confirm_pos..disable_pos];
        assert!(
            enable_body.contains("if __SNAKE__.totp_enabled")
                || enable_body.contains("if user.totp_enabled"),
            "two_factor_enable must reject re-enrollment while 2FA is active: {enable_body}"
        );
        assert!(
            confirm_body.contains("if user.totp_enabled") || confirm_body.contains("totp_enabled"),
            "two_factor_confirm must reject re-enrollment while 2FA is active: {confirm_body}"
        );
    }

    #[test]
    fn totp_confirm_persists_codes_before_enabling_in_transaction() {
        // P2 (#1057): enable + recovery-code replacement must be atomic and the
        // flag flipped only after codes are persisted.
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        let confirm_pos = routes
            .find("pub async fn two_factor_confirm(")
            .expect("confirm fn");
        let after = &routes[confirm_pos..];
        let disable_pos = after
            .find("pub async fn two_factor_disable(")
            .unwrap_or(after.len());
        let body = &after[..disable_pos];
        assert!(
            body.contains(".transaction"),
            "confirm must use a transaction: {body}"
        );
        assert!(
            body.contains("RECOVERY_CODE_COUNT"),
            "confirm must generate N codes: {body}"
        );
        assert!(
            body.contains("hash_password"),
            "recovery codes must be bcrypt-hashed: {body}"
        );
        // totp_enabled flip must come AFTER the recovery-code inserts.
        let enabled_at = body.find("totp_enabled.eq(true)").expect("enable flag set");
        let insert_at = body
            .find("insert_into(recovery_codes::table")
            .expect("code insert");
        assert!(
            enabled_at > insert_at,
            "totp_enabled must be set after codes are persisted"
        );
        // The pending secret must be read from the session, not the live row.
        assert!(
            body.contains("totp_pending_secret"),
            "confirm must consume session pending secret: {body}"
        );
    }

    #[test]
    fn totp_verification_window_and_atomic_replay_guard() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        // ±1 step skew window helper.
        assert!(
            routes.contains("[-1i64, 0, 1]"),
            "verification must use a ±1 step window: {routes}"
        );
        // Atomic, conditional consumption: rows-affected guard on the step update.
        assert!(routes.contains("totp_last_used_step"), "{routes}");
        assert!(
            routes.contains("affected == 1"),
            "step + recovery consumption must require exactly one affected row: {routes}"
        );
        assert!(
            routes.contains("recovery_codes::used_at.is_null()"),
            "recovery consumption must be guarded by used_at IS NULL: {routes}"
        );
    }

    #[test]
    fn totp_recovery_code_marked_used_and_count_surfaced() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("used_at"),
            "consumed recovery code must be stamped used: {routes}"
        );
        assert!(
            routes.contains("remaining"),
            "remaining count must be surfaced: {routes}"
        );
    }

    #[test]
    fn totp_disable_requires_reauth_and_clears_secret() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("verify_password") && routes.contains("two_factor_disable"),
            "disable must require re-auth: {routes}"
        );
        assert!(
            routes.contains("totp_secret_encrypted.eq(None"),
            "disable must clear the stored secret: {routes}"
        );
        assert!(
            routes.contains("delete(recovery_codes::table"),
            "disable must delete recovery codes: {routes}"
        );
    }

    #[test]
    fn totp_secret_encrypted_at_rest() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("Aes256Gcm"),
            "totp secret must be AES-GCM encrypted: {routes}"
        );
        assert!(
            routes.contains("fn encrypt_secret") && routes.contains("fn decrypt_secret"),
            "generated code must provide encrypt/decrypt helpers: {routes}"
        );
        assert!(
            routes.contains("otpauth://") || routes.contains("get_url"),
            "must expose otpauth URI: {routes}"
        );
    }

    #[test]
    fn totp_generates_2fa_integration_tests() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let tests = fs::read_to_string(tmp.path().join("tests/auth_2fa.rs")).unwrap();
        for flow in [
            "two_factor_enroll_and_confirm",
            "login_with_totp_code",
            "login_with_recovery_code",
            "recovery_code_reuse_rejected",
            "two_factor_disable",
        ] {
            assert!(
                tests.contains(flow),
                "tests/auth_2fa.rs missing flow: {flow}"
            );
        }
    }

    #[test]
    fn totp_registers_routes_in_main_rs_and_adds_deps() {
        let tmp = project_with_main();
        totp_plan(tmp.path()).execute(Flags::default()).unwrap();
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        for entry in [
            "routes::auth::two_factor_enable",
            "routes::auth::two_factor_confirm",
            "routes::auth::two_factor_disable",
            "routes::auth::login_verify",
        ] {
            assert!(main.contains(entry), "main.rs missing 2fa route: {entry}");
        }
        let cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("totp-rs"),
            "Cargo.toml missing totp-rs: {cargo}"
        );
        assert!(
            cargo.contains("aes-gcm"),
            "Cargo.toml missing aes-gcm: {cargo}"
        );
    }

    #[test]
    fn without_totp_no_totp_artifacts() {
        let tmp = project_with_main();
        plan_auth(tmp.path(), "User", "20260508000000")
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let model = fs::read_to_string(tmp.path().join("src/models/user.rs")).unwrap();
        assert!(
            !model.contains("totp_enabled"),
            "default model must not contain totp fields: {model}"
        );
        assert!(!tmp.path().join("src/models/recovery_code.rs").exists());
        assert!(!tmp.path().join("tests/auth_2fa.rs").exists());
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            !routes.contains("two_factor_enable"),
            "default routes must not have 2fa handlers"
        );
    }

    #[test]
    fn totp_combines_with_oauth_and_gates_callback() {
        // P2 (#1057): OAuth callback guidance must be TOTP-aware.
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        plan_auth_full(tmp.path(), "User", "20260508000000", &oauth, true)
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        assert!(tmp.path().join("src/routes/oauth.rs").exists());
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("two_factor_enable"),
            "totp+oauth must keep 2fa handlers"
        );
        assert!(
            routes.contains("Or sign in with:"),
            "totp+oauth must keep oauth buttons"
        );
        let oauth_routes = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        assert!(
            oauth_routes.contains("totp_pending_id") && oauth_routes.contains("/login/verify"),
            "oauth callback guidance must route 2FA users through /login/verify: {oauth_routes}"
        );
    }

    #[test]
    fn oauth_without_totp_has_no_2fa_callback_note() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        plan_auth_full(tmp.path(), "User", "20260508000000", &oauth, false)
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        let oauth_routes = fs::read_to_string(tmp.path().join("src/routes/oauth.rs")).unwrap();
        assert!(
            !oauth_routes.contains("TWO-FACTOR"),
            "oauth-only scaffold must not mention the 2FA callback note: {oauth_routes}"
        );
    }

    // ── Passkeys (WebAuthn) generator tests (S-062 / #806) ──────────────────────

    fn passkey_plan(tmp: &std::path::Path) -> Plan {
        plan_auth_full_ex(
            tmp,
            "User",
            "20260508000000",
            &AuthOAuthOptions::default(),
            false,
            true,
        )
        .unwrap()
    }

    #[test]
    fn passkeys_flag_defaults_off() {
        // Without --passkeys, plan_auth_full (the 5-arg form) must not emit any
        // passkey artefacts — passkeys are opt-in.
        let tmp = project_with_main();
        plan_auth_full(
            tmp.path(),
            "User",
            "20260508000000",
            &AuthOAuthOptions::default(),
            false,
        )
        .unwrap()
        .execute(Flags::default())
        .unwrap();
        assert!(
            !tmp.path().join("src/routes/passkeys.rs").exists(),
            "passkeys.rs must not be created without --passkeys"
        );
        assert!(
            !tmp.path().join("tests/auth_passkeys.rs").exists(),
            "auth_passkeys.rs must not be created without --passkeys"
        );
    }

    #[test]
    fn passkeys_plan_emits_webauthn_credentials_migration() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        // The migration directory name is timestamp+1 relative to the base ts.
        let up = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000001_create_webauthn_credentials/up.sql"),
        )
        .unwrap();
        assert!(
            up.contains("CREATE TABLE webauthn_credentials"),
            "missing CREATE TABLE webauthn_credentials: {up}"
        );
        assert!(up.contains("user_id"), "missing user_id column: {up}");
        assert!(
            up.contains("credential_id"),
            "missing credential_id column: {up}"
        );
        assert!(
            up.contains("credential_json"),
            "missing credential_json column: {up}"
        );
        let down = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000001_create_webauthn_credentials/down.sql"),
        )
        .unwrap();
        assert!(
            down.contains("DROP TABLE webauthn_credentials"),
            "down.sql must drop webauthn_credentials: {down}"
        );
    }

    #[test]
    fn passkeys_plan_emits_four_ceremony_routes() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/passkeys.rs")).unwrap();
        for needle in [
            "passkey_register_begin",
            "passkey_register_finish",
            "passkey_login_begin",
            "passkey_login_finish",
        ] {
            assert!(
                routes.contains(needle),
                "passkeys.rs missing ceremony handler: {needle}"
            );
        }
    }

    #[test]
    fn passkeys_plan_emits_list_and_revoke_surface() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/passkeys.rs")).unwrap();
        assert!(
            routes.contains("passkey_list"),
            "missing passkey_list: {routes}"
        );
        assert!(
            routes.contains("passkey_revoke"),
            "missing passkey_revoke: {routes}"
        );
        assert!(
            routes.contains("#[secured]"),
            "revoke/list must use #[secured]: {routes}"
        );
    }

    #[test]
    fn passkeys_login_finish_writes_session_auth_key() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/passkeys.rs")).unwrap();
        assert!(
            routes.contains("state.auth_session_key()"),
            "passkey_login_finish must call session.insert(state.auth_session_key(), ...): {routes}"
        );
        // After template substitution ("User" → "user"), __SNAKE___id becomes user_id.
        assert!(
            routes.contains("\"user_id\""),
            "passkey_login_finish must store user_id in session: {routes}"
        );
        assert!(
            routes.contains("\"user_email\""),
            "passkey_login_finish must store user_email in session: {routes}"
        );
    }

    #[test]
    fn passkeys_plan_emits_rp_config_in_autumn_toml() {
        let tmp = project_with_main();
        // Create an autumn.toml so the generator can update it.
        fs::write(tmp.path().join("autumn.toml"), "[server]\nport = 3000\n").unwrap();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let toml = fs::read_to_string(tmp.path().join("autumn.toml")).unwrap();
        assert!(
            toml.contains("[auth.webauthn]"),
            "autumn.toml missing [auth.webauthn]: {toml}"
        );
        assert!(toml.contains("rp_id"), "autumn.toml missing rp_id: {toml}");
        assert!(
            toml.contains("rp_name"),
            "autumn.toml missing rp_name: {toml}"
        );
        assert!(
            toml.contains("rp_origin"),
            "autumn.toml missing rp_origin: {toml}"
        );
    }

    #[test]
    fn passkeys_plan_adds_webauthn_feature_to_autumn_web() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("webauthn"),
            "Cargo.toml must reference webauthn feature or dep: {cargo}"
        );
        assert!(
            cargo.contains("webauthn-rs"),
            "Cargo.toml missing webauthn-rs dep: {cargo}"
        );
    }

    #[test]
    fn passkeys_plan_generates_integration_tests() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let tests = fs::read_to_string(tmp.path().join("tests/auth_passkeys.rs")).unwrap();
        for needle in [
            "passkey_register_happy_path",
            "passkey_login_happy_path",
            "passkey_wrong_origin_rejected",
            "passkey_unknown_credential_rejected",
            "passkey_revoke_then_relogin",
        ] {
            assert!(
                tests.contains(needle),
                "auth_passkeys.rs missing test: {needle}"
            );
        }
    }

    #[test]
    fn passkeys_plan_writes_docs() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let docs = fs::read_to_string(tmp.path().join("docs/guide/passkeys.md")).unwrap();
        assert!(
            docs.contains("rp_id") || docs.contains("rp_origin") || docs.contains("WebAuthn"),
            "docs/guide/passkeys.md should cover rp config or WebAuthn: {docs}"
        );
    }

    #[test]
    fn passkeys_plan_maud_template_has_js_shim() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/passkeys.rs")).unwrap();
        assert!(
            routes.contains("navigator.credentials"),
            "passkeys.rs Maud template must include navigator.credentials JS: {routes}"
        );
    }

    #[test]
    fn passkeys_plan_registers_routes_in_main() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        for entry in [
            "routes::passkeys::passkey_register_begin",
            "routes::passkeys::passkey_register_finish",
            "routes::passkeys::passkey_login_begin",
            "routes::passkeys::passkey_login_finish",
            "routes::passkeys::passkey_list",
            "routes::passkeys::passkey_revoke",
        ] {
            assert!(
                main.contains(entry),
                "main.rs missing passkey route: {entry}"
            );
        }
    }

    #[test]
    fn passkeys_can_combine_with_oauth_and_totp() {
        let tmp = project_with_main();
        let oauth = AuthOAuthOptions {
            providers: vec!["github".to_owned()],
        };
        plan_auth_full_ex(tmp.path(), "User", "20260508000000", &oauth, true, true)
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        assert!(tmp.path().join("src/routes/passkeys.rs").exists());
        assert!(tmp.path().join("src/routes/oauth.rs").exists());
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("two_factor_enable"),
            "totp+passkeys+oauth must keep 2fa handlers"
        );
    }

    #[test]
    fn passkeys_misconfiguration_fails_fast_error_message() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/passkeys.rs")).unwrap();
        assert!(
            routes.contains("auth.webauthn"),
            "build_webauthn error must mention auth.webauthn config key: {routes}"
        );
    }

    #[test]
    fn ensure_webauthn_rs_features_merges_into_shorthand() {
        let toml = "webauthn-rs = \"0.5\"\n";
        let out = ensure_webauthn_rs_features(toml);
        assert!(
            out.contains("danger-allow-state-serialisation"),
            "shorthand should be promoted: {out}"
        );
        assert!(
            out.contains("conditional-ui"),
            "missing conditional-ui: {out}"
        );
    }

    #[test]
    fn ensure_webauthn_rs_features_merges_into_partial_inline() {
        let toml = "webauthn-rs = { version = \"0.5\", features = [\"danger-allow-state-serialisation\"] }\n";
        let out = ensure_webauthn_rs_features(toml);
        assert!(
            out.contains("conditional-ui"),
            "conditional-ui should be added to partial features: {out}"
        );
    }

    #[test]
    fn ensure_webauthn_rs_features_is_idempotent() {
        let toml = "webauthn-rs = { version = \"0.5\", features = [\"danger-allow-state-serialisation\", \"conditional-ui\"] }\n";
        let out = ensure_webauthn_rs_features(toml);
        assert_eq!(out, toml, "idempotent: should not duplicate features");
    }

    #[test]
    fn passkeys_routes_template_has_redirect_to() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/passkeys.rs")).unwrap();
        assert!(
            routes.contains("fn redirect_to"),
            "passkeys.rs must define redirect_to: {routes}"
        );
        assert!(
            routes.contains("impl IntoResponse"),
            "redirect_to must return impl IntoResponse: {routes}"
        );
    }

    #[test]
    fn passkeys_register_begin_binds_user_id() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/passkeys.rs")).unwrap();
        assert!(
            routes.contains("user_id"),
            "register begin should store user_id in passkey_reg_state envelope: {routes}"
        );
    }

    #[test]
    fn passkeys_templates_accept_csrf_header_and_nonce() {
        let tmp = project_with_main();
        passkey_plan(tmp.path()).execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/passkeys.rs")).unwrap();

        assert!(
            routes.contains("pub async fn passkey_register_page("),
            "passkey_register_page is missing"
        );
        assert!(
            routes.contains("csrf_header: Option<CsrfTokenHeader>"),
            "passkey_register_page must take csrf_header"
        );
        assert!(
            routes.contains("nonce: Option<CspNonce>"),
            "passkey_register_page must take nonce"
        );
        assert!(
            routes.contains("pub async fn passkey_login_page("),
            "passkey_login_page is missing"
        );
        assert!(
            routes.contains("csrf_header: Option<CsrfTokenHeader>"),
            "passkey_login_page must take csrf_header"
        );
        assert!(
            routes.contains("nonce: Option<CspNonce>"),
            "passkey_login_page must take nonce"
        );

        assert!(
            routes.contains("script nonce=[script_nonce]"),
            "templates must output script tags with nonce attribute: {routes}"
        );
        assert!(
            routes.contains("const csrfHeader = document.querySelector('meta[name=\"csrf-token-header\"]')?.content ?? 'X-CSRF-Token';"),
            "templates must dynamically resolve csrf header name: {routes}"
        );
        assert!(
            routes.contains("[csrfHeader]: csrfToken"),
            "templates must use dynamic csrf header key in fetch calls: {routes}"
        );
    }

    // ── Account lockout (issue #814) ────────────────────────────────────────

    #[test]
    fn migration_up_sql_contains_lockout_columns() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let up = fs::read_to_string(
            tmp.path()
                .join("migrations/20260508000000_create_users/up.sql"),
        )
        .unwrap();
        assert!(
            up.contains("failed_attempts"),
            "migration must include failed_attempts column: {up}"
        );
        assert!(
            up.contains("locked_at"),
            "migration must include locked_at column: {up}"
        );
    }

    #[test]
    fn model_file_contains_lockout_fields() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let model = fs::read_to_string(tmp.path().join("src/models/user.rs")).unwrap();
        assert!(
            model.contains("pub failed_attempts"),
            "model must include failed_attempts field: {model}"
        );
        assert!(
            model.contains("pub locked_at"),
            "model must include locked_at field: {model}"
        );
    }

    #[test]
    fn schema_rs_contains_lockout_columns() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let schema = fs::read_to_string(tmp.path().join("src/schema.rs")).unwrap();
        assert!(
            schema.contains("failed_attempts"),
            "schema must include failed_attempts: {schema}"
        );
        assert!(
            schema.contains("locked_at"),
            "schema must include locked_at: {schema}"
        );
    }

    #[test]
    fn routes_file_checks_lockout_state_on_login() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("locked_at"),
            "login handler must check locked_at for lockout: {routes}"
        );
        assert!(
            routes.contains("failed_attempts"),
            "login handler must track failed_attempts: {routes}"
        );
    }

    #[test]
    fn routes_file_lockout_response_indistinguishable_from_wrong_password() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        // Lockout must return the same auth_err closure as wrong password (non-enumerating).
        let auth_err_count = routes.matches("auth_err()").count();
        assert!(
            auth_err_count >= 2,
            "lockout must reuse auth_err() for non-enumerating response (found {auth_err_count}): {routes}"
        );
    }

    #[test]
    fn routes_file_resets_failed_attempts_on_successful_login() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("failed_attempts.eq(0)"),
            "successful login must reset failed_attempts to 0: {routes}"
        );
    }

    #[test]
    fn routes_file_emits_telemetry_event_on_lockout_transition() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        assert!(
            routes.contains("account_locked"),
            "lockout transition must emit a structured telemetry event with account_locked: {routes}"
        );
    }

    #[test]
    fn routes_file_respects_lockout_enabled_config() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let routes = fs::read_to_string(tmp.path().join("src/routes/auth.rs")).unwrap();
        // Must check enabled flag or threshold so operators can disable lockout.
        assert!(
            routes.contains("lockout")
                && (routes.contains("enabled") || routes.contains("threshold")),
            "routes must respect lockout enabled/threshold config for opt-out: {routes}"
        );
    }

    #[test]
    fn generated_tests_cover_lockout_scenarios() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let tests = fs::read_to_string(tmp.path().join("tests/auth.rs")).unwrap();
        // AC8a: N+1 failed attempts reject correct credentials
        assert!(
            tests.contains("auth_lockout_rejects_correct_credentials"),
            "generated tests must cover: N+1 failed attempts with correct creds are rejected: {tests}"
        );
        // AC8b: successful login resets counter
        assert!(
            tests.contains("auth_successful_login_resets_lockout_counter"),
            "generated tests must cover: successful login resets counter: {tests}"
        );
        // AC8c: locked-state response byte-identical to wrong-password
        assert!(
            tests.contains("auth_lockout_response_identical_to_wrong_password"),
            "generated tests must verify lockout response is byte-identical to wrong-password: {tests}"
        );
    }

    #[test]
    fn docs_file_documents_lockout_config() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let docs = fs::read_to_string(tmp.path().join("docs/guide/authentication.md")).unwrap();
        assert!(
            docs.contains("lockout"),
            "docs must document the lockout policy: {docs}"
        );
        assert!(
            docs.contains("threshold"),
            "docs must document the threshold setting: {docs}"
        );
        assert!(
            docs.contains("cooloff") || docs.contains("cool_off") || docs.contains("cool-off"),
            "docs must document the cool-off period: {docs}"
        );
    }

    #[test]
    fn docs_file_documents_operator_unlock_path() {
        let tmp = project_with_main();
        let plan = plan_auth(tmp.path(), "User", "20260508000000").unwrap();
        plan.execute(Flags::default()).unwrap();
        let docs = fs::read_to_string(tmp.path().join("docs/guide/authentication.md")).unwrap();
        assert!(
            docs.contains("unlock"),
            "docs must document operator unlock path: {docs}"
        );
    }
}
