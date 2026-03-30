use autumn_web::cached;
use autumn_web::error::{AutumnError, AutumnResult};

#[cached(result)]
async fn get_user(id: i64) -> AutumnResult<String> {
    if id < 0 {
        return Err(AutumnError::bad_request_msg("invalid id"));
    }
    Ok(format!("user-{id}"))
}

#[cached(ttl = "30s", max = 200, result)]
async fn fetch_data(key: String) -> AutumnResult<Vec<u8>> {
    Ok(key.into_bytes())
}

fn main() {}
