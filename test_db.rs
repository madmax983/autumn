use autumn_web::{get, routes};

#[get("/db")]
async fn test_db(db: autumn_web::extract::Db) -> String {
    "Db connected".to_string()
}
