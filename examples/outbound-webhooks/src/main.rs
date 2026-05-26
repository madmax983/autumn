#[autumn_web::main]
async fn main() {
    let app = outbound_webhooks_example::app();
    app.run().await;
}
