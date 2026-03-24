mod models;
mod routes;
mod schema;

use autumn::routes;

#[autumn::main]
async fn main() {
    autumn::app()
        .routes(routes![
            routes::todos::index,
            routes::todos::list,
            routes::todos::detail,
            routes::todos::create,
            routes::todos::toggle,
            routes::todos::delete_todo,
            routes::api::list_json,
            routes::api::create_json,
        ])
        .run()
        .await;
}
