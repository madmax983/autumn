use autumn_web::form::{form_tag, submit_button, text_input};
use autumn_web::prelude::*;
use serde::{Deserialize, Serialize};
use validator::Validate;

#[get("/")]
async fn index() -> &'static str {
    "Welcome to Autumn!"
}

#[get("/hello")]
async fn hello() -> &'static str {
    "Hello, Autumn!"
}

#[get("/hello/{name}")]
async fn hello_name(name: Path<String>) -> String {
    format!("Hello, {}!", *name)
}

// ── Form showcase ────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Validate, Clone)]
struct GreetForm {
    #[validate(length(min = 2, max = 50, message = "Name must be 2–50 characters"))]
    name: String,
    #[validate(email(message = "Must be a valid email address"))]
    email: String,
}

#[get("/greet/new")]
async fn new_greet() -> Markup {
    let cs = Changeset::new(GreetForm { name: String::new(), email: String::new() });
    form_tag("/greet", "post", None, html! {
        (text_input(&cs, "name", "Your name"))
        (text_input(&cs, "email", "Email address"))
        (submit_button("Say hello"))
    })
}

#[post("/greet")]
async fn create_greet(form: ChangesetForm<GreetForm>) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;
    match form.into_valid() {
        Ok(g) => (StatusCode::OK, html! { p { "Hello, " (g.name) "!" } }),
        Err(form) => {
            let cs = form.into_changeset();
            (StatusCode::UNPROCESSABLE_ENTITY, form_tag("/greet", "post", None, html! {
                (text_input(&cs, "name", "Your name"))
                (text_input(&cs, "email", "Email address"))
                (submit_button("Say hello"))
            }))
        }
    }
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![index, hello, hello_name, new_greet, create_greet])
        .run()
        .await;
}
