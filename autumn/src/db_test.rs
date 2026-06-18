use autumn::db::is_query_canceled;
use diesel::result::Error;

fn main() {
    let _ = is_query_canceled(&Error::RollbackTransaction);
}
