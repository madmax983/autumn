use autumn_web::cached;

struct MyService;

impl MyService {
    #[cached]
    async fn get_thing(&self) -> String {
        "hi".into()
    }
}

fn main() {}
