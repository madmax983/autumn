use autumn_web::extract::{Form, Json};
use autumn_web::{get, html, post, AutumnResult, Markup};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Item {
    name: String,
}

#[derive(Deserialize)]
struct NewItem {
    name: String,
}

#[derive(Deserialize)]
struct LoginForm {
    user: String,
}

#[get("/items")]
async fn list_items() -> Json<Vec<Item>> {
    Json(vec![])
}

#[post("/items")]
async fn create_item(Json(_body): Json<NewItem>) -> AutumnResult<Json<Item>> {
    Ok(Json(Item {
        name: "test".into(),
    }))
}

#[post("/login")]
async fn login(Form(_data): Form<LoginForm>) -> &'static str {
    "ok"
}

#[get("/page")]
async fn page() -> Markup {
    html! { p { "hello" } }
}

#[get("/fallible")]
async fn fallible_page() -> AutumnResult<Markup> {
    Ok(html! { p { "ok" } })
}

fn main() {}
