//! `autumn generate mailer` — scaffold a `#[mailer]` struct, HTML+text templates,
//! preview registration, and a smoke test.
//!
//! For a name like `Welcome`, the generator produces:
//! - `src/mailers/welcome.rs` — `WelcomeMailer` struct with a `#[mailer]` impl.
//!   The macro generates `send_welcome` (async) and `deliver_later_welcome`
//!   (fire-and-forget) from each method.
//! - `src/mailers/previews/welcome.rs` — dev-only `#[mailer_preview]` impl,
//!   kept separate from production code (mirrors Rails `test/mailers/previews/`).
//! - `src/mailers/previews/mod.rs` — created or updated with `pub mod welcome;`.
//! - `templates/mailers/welcome.html` — HTML template placeholder.
//! - `templates/mailers/welcome.txt` — plain-text template placeholder.
//! - `src/mailers/mod.rs` — created or updated with `pub mod welcome;` and
//!   `pub mod previews;`.
//! - `tests/welcome_mailer.rs` — smoke test asserting both bodies render.
//! - `src/main.rs` — `mod mailers;` declaration and
//!   `.mail_previews(mail_previews![mailers::welcome::WelcomeMailer])` wired
//!   into the app builder chain.
//! - `Cargo.toml` — `"mail"` feature added to the `autumn-web` dependency.

use std::path::Path;

use super::emit::Plan;
use super::model::validate_resource_name;
use super::naming::{pascal, snake};
use super::schema_edit::{
    add_mail_preview_to_app, add_mod_declaration, ensure_autumn_web_feature, update_main_rs,
};
use super::{Flags, GenerateError, ensure_project_root};

/// Compute the file actions for `autumn generate mailer`.
///
/// # Errors
/// Project layout and name validation errors surface here.
pub fn plan_mailer(project_root: &Path, name: &str) -> Result<Plan, GenerateError> {
    ensure_project_root(project_root)?;
    validate_resource_name(name)?;

    let snake_name = snake(name);
    let pascal_name = pascal(name);
    let struct_name = format!("{pascal_name}Mailer");
    let mailer_type = format!("mailers::{snake_name}::{struct_name}");

    let mut plan = Plan::new(project_root);

    // ── src/mailers/<snake>.rs ─────────────────────────────────────────────
    plan.create(
        project_root
            .join("src")
            .join("mailers")
            .join(format!("{snake_name}.rs")),
        render_mailer_file(&struct_name, &snake_name),
    );

    // ── templates/mailers/<snake>.html ─────────────────────────────────────
    plan.create(
        project_root
            .join("templates")
            .join("mailers")
            .join(format!("{snake_name}.html")),
        render_html_template(&struct_name),
    );

    // ── templates/mailers/<snake>.txt ──────────────────────────────────────
    plan.create(
        project_root
            .join("templates")
            .join("mailers")
            .join(format!("{snake_name}.txt")),
        render_txt_template(&struct_name),
    );

    // ── src/mailers/previews/<snake>.rs ────────────────────────────────────
    plan.create(
        project_root
            .join("src")
            .join("mailers")
            .join("previews")
            .join(format!("{snake_name}.rs")),
        render_preview_file(&struct_name, &snake_name),
    );

    // ── src/mailers/previews/mod.rs (create or update) ─────────────────────
    let previews_mod_path = project_root
        .join("src")
        .join("mailers")
        .join("previews")
        .join("mod.rs");
    plan.modify(
        previews_mod_path.clone(),
        add_mod_declaration(&read_or_empty(&previews_mod_path), &snake_name),
    );

    // ── src/mailers/mod.rs (create or update) ──────────────────────────────
    let mod_path = project_root.join("src").join("mailers").join("mod.rs");
    let with_mailer_mod = add_mod_declaration(&read_or_empty(&mod_path), &snake_name);
    let with_both_mods = add_mod_declaration(&with_mailer_mod, "previews");
    plan.modify(mod_path, with_both_mods);


    // ── src/main.rs: add mod mailers; and .mail_previews(…) ────────────────
    let main_path = project_root.join("src").join("main.rs");
    let main_existing = std::fs::read_to_string(&main_path).map_err(|_| {
        GenerateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing {}", main_path.display()),
        ))
    })?;
    let with_mods = update_main_rs(&main_existing, &["mailers"], &[]);
    let updated_main = add_mail_preview_to_app(&with_mods, &mailer_type);
    plan.modify(main_path, updated_main);

    // ── Cargo.toml: ensure autumn-web has the "mail" feature ───────────────
    let cargo_path = project_root.join("Cargo.toml");
    let cargo_existing = read_or_empty(&cargo_path);
    let updated_cargo = ensure_autumn_web_feature(&cargo_existing, "mail");
    if updated_cargo != cargo_existing {
        plan.modify(cargo_path, updated_cargo);
    }

    Ok(plan)
}

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn render_mailer_file(struct_name: &str, snake_name: &str) -> String {
    let base = format!(
        r#"//! Generated by `autumn generate mailer`.
//!
//! Edit freely — once generated, this is ordinary user code.
//!
//! The `#[mailer]` macro generates two helpers for every method:
//!   - `send_{snake_name}` — async, awaits delivery.
//!   - `deliver_later_{snake_name}` — fire-and-forget (uses `deliver_later` semantics).
//!
//! Dev-only preview fixtures live in `src/mailers/previews/{snake_name}.rs`.

use autumn_web::prelude::*;

pub struct {struct_name};

#[mailer]
impl {struct_name} {{
    pub fn {snake_name}(&self, to: String) -> Mail {{
        Mail::builder()
            .to(to)
            .subject("{struct_name}")
            .html(include_str!("../../templates/mailers/{snake_name}.html"))
            .text(include_str!("../../templates/mailers/{snake_name}.txt"))
            .build()
            .expect("valid mail")
    }}
}}
"#
    );
    format!("{}{}", base, render_smoke_test(struct_name, snake_name))
}

fn render_preview_file(struct_name: &str, snake_name: &str) -> String {
    format!(
        r#"//! Dev-only mail preview for [`{struct_name}`].
//!
//! Generated by `autumn generate mailer`. Edit freely.
//!
//! The `#[mailer_preview]` macro exposes preview fixtures to the Autumn
//! dev UI at `/_autumn/mail`. Methods must be zero-argument and return `Mail`.

use autumn_web::prelude::*;
use crate::mailers::{snake_name}::{struct_name};

#[mailer_preview]
impl {struct_name} {{
    fn {snake_name}_preview() -> Mail {{
        Mail::builder()
            .to("preview@example.com")
            .subject("{struct_name}")
            .html(include_str!("../../../templates/mailers/{snake_name}.html"))
            .text(include_str!("../../../templates/mailers/{snake_name}.txt"))
            .build()
            .expect("valid preview mail")
    }}
}}
"#
    )
}

fn render_html_template(struct_name: &str) -> String {
    format!(
        "<!DOCTYPE html>\n\
         <html>\n\
         <head><meta charset=\"utf-8\"></head>\n\
         <body>\n\
           <p>Hello from {struct_name}!</p>\n\
         </body>\n\
         </html>\n"
    )
}

fn render_txt_template(struct_name: &str) -> String {
    format!("Hello from {struct_name}!\n")
}

fn render_smoke_test(struct_name: &str, snake_name: &str) -> String {
    format!(
        "\n\
         #[cfg(test)]\n\
         mod {snake_name}_mailer_tests {{\n\
             use super::{struct_name};\n\
         \n\
             #[test]\n\
             fn {snake_name}_mailer_renders_both_bodies() {{\n\
                 let mailer_instance = {struct_name};\n\
                 let mail = mailer_instance.{snake_name}(\"test@example.com\".to_owned());\n\
                 let html = mail.html.expect(\"html body should be set\");\n\
                 let text = mail.text.expect(\"text body should be set\");\n\
                 assert!(\n\
                     html.contains(\"{struct_name}\"),\n\
                     \"html body must contain the mailer name; got: {{html}}\"\n\
                 );\n\
                 assert!(\n\
                     text.contains(\"{struct_name}\"),\n\
                     \"text body must contain the mailer name; got: {{text}}\"\n\
                 );\n\
             }}\n\
         }}\n"
    )
}

/// CLI entry point.
pub fn run(name: &str, flags: Flags) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    match plan_mailer(&cwd, name).and_then(|p| p.execute(flags)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn project_with_main(main_content: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\n\n[dependencies]\nautumn-web = \"0.6\"\n",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), main_content).unwrap();
        tmp
    }

    fn default_main() -> &'static str {
        r#"use autumn_web::prelude::*;

#[get("/")]
async fn index() -> &'static str { "ok" }

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index])
        .run()
        .await;
}
"#
    }

    // ── RED: file plan assertions ─────────────────────────────────────────

    #[test]
    fn plan_creates_mailer_file() {
        let tmp = project_with_main(default_main());
        let plan = plan_mailer(tmp.path(), "Welcome").unwrap();
        assert!(
            plan.actions
                .iter()
                .any(|a| a.path().ends_with("src/mailers/welcome.rs")),
            "plan must include src/mailers/welcome.rs"
        );
    }

    #[test]
    fn plan_creates_preview_file() {
        let tmp = project_with_main(default_main());
        let plan = plan_mailer(tmp.path(), "Welcome").unwrap();
        assert!(
            plan.actions
                .iter()
                .any(|a| a.path().ends_with("src/mailers/previews/welcome.rs")),
            "plan must include src/mailers/previews/welcome.rs"
        );
    }

    #[test]
    fn plan_includes_previews_mod_rs() {
        let tmp = project_with_main(default_main());
        let plan = plan_mailer(tmp.path(), "Welcome").unwrap();
        assert!(
            plan.actions
                .iter()
                .any(|a| a.path().ends_with("src/mailers/previews/mod.rs")),
            "plan must include src/mailers/previews/mod.rs"
        );
    }

    #[test]
    fn plan_creates_html_template() {
        let tmp = project_with_main(default_main());
        let plan = plan_mailer(tmp.path(), "Welcome").unwrap();
        assert!(
            plan.actions
                .iter()
                .any(|a| a.path().ends_with("templates/mailers/welcome.html")),
            "plan must include templates/mailers/welcome.html"
        );
    }

    #[test]
    fn plan_creates_txt_template() {
        let tmp = project_with_main(default_main());
        let plan = plan_mailer(tmp.path(), "Welcome").unwrap();
        assert!(
            plan.actions
                .iter()
                .any(|a| a.path().ends_with("templates/mailers/welcome.txt")),
            "plan must include templates/mailers/welcome.txt"
        );
    }

    #[test]
    fn plan_includes_mailer_mod_rs() {
        let tmp = project_with_main(default_main());
        let plan = plan_mailer(tmp.path(), "Welcome").unwrap();
        assert!(
            plan.actions
                .iter()
                .any(|a| a.path().ends_with("src/mailers/mod.rs")),
            "plan must include src/mailers/mod.rs"
        );
    }

    #[test]
    fn plan_does_not_create_external_smoke_test() {
        let tmp = project_with_main(default_main());
        let plan = plan_mailer(tmp.path(), "Welcome").unwrap();
        assert!(
            !plan.actions
                .iter()
                .any(|a| a.path().to_string_lossy().contains("tests/welcome_mailer.rs")),
            "plan must not include tests/welcome_mailer.rs"
        );
    }

    #[test]
    fn plan_updates_main_rs() {
        let tmp = project_with_main(default_main());
        let plan = plan_mailer(tmp.path(), "Welcome").unwrap();
        assert!(
            plan.actions
                .iter()
                .any(|a| a.path().ends_with("src/main.rs")),
            "plan must update src/main.rs"
        );
    }

    // ── GREEN: execute and inspect written content ────────────────────────

    #[test]
    fn execute_writes_mailer_struct_and_macro_annotations() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let mailer = fs::read_to_string(tmp.path().join("src/mailers/welcome.rs")).unwrap();
        assert!(
            mailer.contains("pub struct WelcomeMailer"),
            "mailer file must define the struct"
        );
        assert!(
            mailer.contains("#[mailer]"),
            "must have #[mailer] attribute"
        );
        assert!(
            !mailer.contains("#[mailer_preview]"),
            "mailer file must NOT contain #[mailer_preview] — it lives in previews/"
        );

        let preview =
            fs::read_to_string(tmp.path().join("src/mailers/previews/welcome.rs")).unwrap();
        assert!(
            preview.contains("#[mailer_preview]"),
            "preview file must have #[mailer_preview] attribute"
        );
    }

    #[test]
    fn execute_mailer_has_deliver_later_capable_method() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let mailer = fs::read_to_string(tmp.path().join("src/mailers/welcome.rs")).unwrap();
        assert!(
            mailer.contains("pub fn welcome("),
            "#[mailer] impl must expose a method; the macro generates deliver_later_welcome from it"
        );
        assert!(
            mailer.contains("deliver_later"),
            "the generated comment must describe the deliver_later API"
        );
    }

    #[test]
    fn execute_writes_html_template_with_expected_content() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let html = fs::read_to_string(tmp.path().join("templates/mailers/welcome.html")).unwrap();
        assert!(
            html.contains("WelcomeMailer"),
            "html template must reference the mailer name"
        );
        assert!(
            html.contains("<!DOCTYPE html>"),
            "html template must be a valid HTML document"
        );
    }

    #[test]
    fn execute_writes_txt_template_with_expected_content() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let txt = fs::read_to_string(tmp.path().join("templates/mailers/welcome.txt")).unwrap();
        assert!(
            txt.contains("WelcomeMailer"),
            "text template must reference the mailer name"
        );
        assert!(!txt.trim().is_empty(), "text template must not be empty");
    }

    #[test]
    fn execute_writes_preview_file_with_mailer_preview() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let preview =
            fs::read_to_string(tmp.path().join("src/mailers/previews/welcome.rs")).unwrap();
        assert!(
            preview.contains("#[mailer_preview]"),
            "preview file must have #[mailer_preview] attribute"
        );
        assert!(
            preview.contains("welcome_preview"),
            "preview file must define a preview method"
        );
        assert!(
            preview.contains("../../../templates/mailers/welcome.html"),
            "preview include_str! path must be three levels up from previews/"
        );
    }

    #[test]
    fn execute_updates_mailer_mod_rs() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let mod_rs = fs::read_to_string(tmp.path().join("src/mailers/mod.rs")).unwrap();
        assert!(
            mod_rs.contains("pub mod welcome;"),
            "mod.rs must declare pub mod welcome"
        );
        assert!(
            mod_rs.contains("pub mod previews;"),
            "mod.rs must declare pub mod previews"
        );

        let previews_mod =
            fs::read_to_string(tmp.path().join("src/mailers/previews/mod.rs")).unwrap();
        assert!(
            previews_mod.contains("pub mod welcome;"),
            "previews/mod.rs must declare pub mod welcome"
        );
    }

    #[test]
    fn execute_updates_main_rs_with_mod_declaration() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(
            main.contains("mod mailers;"),
            "main.rs must declare mod mailers"
        );
    }

    #[test]
    fn execute_updates_main_rs_with_mail_previews_call() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(
            main.contains("mail_previews!["),
            "main.rs must include a mail_previews![] call"
        );
        assert!(
            main.contains("mailers::welcome::WelcomeMailer"),
            "main.rs must reference the generated mailer type"
        );
    }

    #[test]
    fn execute_writes_smoke_test_with_body_assertions() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let test = fs::read_to_string(tmp.path().join("src/mailers/welcome.rs")).unwrap();
        assert!(
            test.contains("WelcomeMailer"),
            "smoke test must reference the mailer struct"
        );
        assert!(
            test.contains("welcome_mailer_renders_both_bodies")
                || test.contains("renders_both_bodies"),
            "smoke test must include a renders-both-bodies test function"
        );
        assert!(
            test.contains("html.contains") || test.contains("html body"),
            "smoke test must assert the html body"
        );
        assert!(
            test.contains("text.contains") || test.contains("text body"),
            "smoke test must assert the text body"
        );
        assert!(
            test.contains("WelcomeMailer"),
            "smoke test body assertion must check for the mailer name"
        );
        assert!(
            !test.contains("cfg(feature = \"mail\")"),
            "smoke test must not be gated by cfg(feature = \"mail\")"
        );
    }

    #[test]
    fn execute_adds_mail_feature_to_cargo_toml() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        let cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
        assert!(
            cargo.contains("\"mail\""),
            "Cargo.toml must include the mail feature for autumn-web: {cargo}"
        );
    }

    // ── Flag behaviour ────────────────────────────────────────────────────

    #[test]
    fn dry_run_writes_no_new_files() {
        let tmp = project_with_main(default_main());
        let original_main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags {
                dry_run: true,
                force: false,
            })
            .unwrap();

        assert!(
            !tmp.path().join("src/mailers/welcome.rs").exists(),
            "dry run must not write the mailer file"
        );
        assert!(
            !tmp.path().join("src/mailers/previews/welcome.rs").exists(),
            "dry run must not write the preview file"
        );
        assert!(
            !tmp.path().join("templates/mailers/welcome.html").exists(),
            "dry run must not write html template"
        );
        assert!(
            !tmp.path().join("templates/mailers/welcome.txt").exists(),
            "dry run must not write txt template"
        );
        let main_after = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert_eq!(
            original_main, main_after,
            "dry run must not modify src/main.rs"
        );
    }

    #[test]
    fn collision_without_force_returns_error() {
        let tmp = project_with_main(default_main());
        fs::create_dir_all(tmp.path().join("src/mailers")).unwrap();
        fs::write(tmp.path().join("src/mailers/welcome.rs"), "// existing").unwrap();
        let err = plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap_err();
        assert!(
            matches!(err, GenerateError::Collisions(_)),
            "should return collision error; got {err:?}"
        );
    }

    #[test]
    fn force_overwrites_existing_mailer() {
        let tmp = project_with_main(default_main());
        fs::create_dir_all(tmp.path().join("src/mailers")).unwrap();
        fs::write(tmp.path().join("src/mailers/welcome.rs"), "// old").unwrap();
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags {
                force: true,
                dry_run: false,
            })
            .unwrap();

        let mailer = fs::read_to_string(tmp.path().join("src/mailers/welcome.rs")).unwrap();
        assert!(
            mailer.contains("WelcomeMailer"),
            "force must regenerate the mailer file"
        );
    }

    // ── Name normalisation ────────────────────────────────────────────────

    #[test]
    fn snake_case_input_is_normalised() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "welcome_email")
            .unwrap()
            .execute(Flags::default())
            .unwrap();

        assert!(tmp.path().join("src/mailers/welcome_email.rs").exists());
        let content = fs::read_to_string(tmp.path().join("src/mailers/welcome_email.rs")).unwrap();
        assert!(
            content.contains("WelcomeEmailMailer"),
            "PascalCase struct must include both words"
        );
    }

    // ── Error conditions ──────────────────────────────────────────────────

    #[test]
    fn plan_errors_when_main_rs_missing() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let err = plan_mailer(tmp.path(), "Welcome").unwrap_err();
        assert!(matches!(err, GenerateError::Io(_)));
    }

    #[test]
    fn plan_errors_when_not_in_project() {
        let tmp = TempDir::new().unwrap();
        let err = plan_mailer(tmp.path(), "Welcome").unwrap_err();
        assert!(matches!(err, GenerateError::NotInProject));
    }

    #[test]
    fn second_mailer_augments_mod_rs_and_previews() {
        let tmp = project_with_main(default_main());
        plan_mailer(tmp.path(), "Welcome")
            .unwrap()
            .execute(Flags::default())
            .unwrap();
        plan_mailer(tmp.path(), "Notification")
            .unwrap()
            .execute(Flags {
                force: true,
                dry_run: false,
            })
            .unwrap();

        let mod_rs = fs::read_to_string(tmp.path().join("src/mailers/mod.rs")).unwrap();
        assert!(mod_rs.contains("pub mod welcome;"));
        assert!(mod_rs.contains("pub mod notification;"));
        assert!(
            mod_rs.contains("pub mod previews;"),
            "previews module must appear once"
        );
        assert_eq!(
            mod_rs.matches("pub mod previews;").count(),
            1,
            "pub mod previews; must appear exactly once (idempotent)"
        );

        let previews_mod =
            fs::read_to_string(tmp.path().join("src/mailers/previews/mod.rs")).unwrap();
        assert!(previews_mod.contains("pub mod welcome;"));
        assert!(previews_mod.contains("pub mod notification;"));

        let main = fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(main.contains("mailers::welcome::WelcomeMailer"));
        assert!(main.contains("mailers::notification::NotificationMailer"));
    }
}
