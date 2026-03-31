use autumn_web::{AutumnResult, actions, get, routes, server};

#[get("/test")]
async fn test_handler() -> &'static str {
    "test"
}

#[get("/other")]
async fn other_handler() -> &'static str {
    "other"
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct RenameInput {
    id: i64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct RenameOutput {
    id: i64,
}

#[server]
async fn rename_todo(input: RenameInput) -> AutumnResult<RenameOutput> {
    Ok(RenameOutput { id: input.id })
}

#[test]
fn app_builder_accepts_routes() {
    let builder = autumn_web::app().routes(routes![test_handler]);
    // Verify it compiles and doesn't panic — we can't call .run()
    // without actually starting a server.
    let _ = builder;
}

#[test]
fn app_builder_multiple_route_calls() {
    let builder = autumn_web::app()
        .routes(routes![test_handler])
        .routes(routes![other_handler]);
    let _ = builder;
}

#[test]
fn app_builder_accepts_actions() {
    let builder = autumn_web::app()
        .routes(routes![test_handler])
        .actions(actions![rename_todo]);
    let _ = builder;
}

#[tokio::test]
#[should_panic(expected = "No routes registered")]
async fn empty_routes_panics() {
    autumn_web::app().run().await;
}
