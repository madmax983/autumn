use autumn_openapi_plugin::OpenApiPlugin;
use autumn_web::prelude::*;

#[api_doc(summary = "Get greeting", tag = "greeting")]
#[get("/hello")]
async fn hello() -> &'static str {
    "Hello World"
}

#[api_doc(summary = "Get user by ID", tag = "users")]
#[get("/users/{id}")]
async fn get_user(Path(id): Path<i32>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": id,
        "name": "Nova"
    }))
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .plugin(OpenApiPlugin::new("My API", "v1.0").path("/swagger"))
        .routes(routes![hello, get_user])
        .run()
        .await;
}
