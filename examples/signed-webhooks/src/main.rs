#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(signed_webhooks_example::routes())
        .run()
        .await;
}
