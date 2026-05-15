mod models;
mod routes;
mod schema;

use autumn_web::routes;

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(routes![
            routes::api_list,
            routes::api_show,
            routes::api_create,
            routes::api_update,
            routes::api_delete,
            routes::api_protected,
            routes::html_list,
            routes::html_show,
        ])
        .run()
        .await;
}

#[cfg(test)]
mod tests {
    use super::routes::*;
    use crate::models::NewPost;

    fn valid_new_post() -> NewPost {
        NewPost {
            title: "Test Title".into(),
            body: "Test body content.".into(),
            published: false,
            author: "tester".into(),
        }
    }

    #[test]
    fn validation_rejects_blank_title() {
        let mut p = valid_new_post();
        p.title = "   ".into();
        assert!(super::routes::validate_new_post_pub(&p).is_err());
    }

    #[test]
    fn validation_rejects_long_title() {
        let mut p = valid_new_post();
        p.title = "x".repeat(256);
        assert!(super::routes::validate_new_post_pub(&p).is_err());
    }

    #[test]
    fn validation_rejects_blank_body() {
        let mut p = valid_new_post();
        p.body = String::new();
        assert!(super::routes::validate_new_post_pub(&p).is_err());
    }

    #[test]
    fn validation_rejects_blank_author() {
        let mut p = valid_new_post();
        p.author = String::new();
        assert!(super::routes::validate_new_post_pub(&p).is_err());
    }

    #[test]
    fn validation_accepts_valid_post() {
        let p = valid_new_post();
        assert!(super::routes::validate_new_post_pub(&p).is_ok());
    }
}
