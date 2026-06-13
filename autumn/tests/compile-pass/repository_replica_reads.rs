// Compile-pass: replica-aware read routing (#971) — the `primary_reads`
// per-repository opt-out and the per-call `on_primary()` escape hatch.

mod schema {
    autumn_web::reexports::diesel::table! {
        articles (id) {
            id -> Int8,
            title -> Text,
        }
    }

    autumn_web::reexports::diesel::table! {
        ledgers (id) {
            id -> Int8,
            title -> Text,
        }
    }
}

use schema::{articles, ledgers};

#[autumn_web::model(table = "articles")]
pub struct Article {
    #[id]
    pub id: i64,
    pub title: String,
}

// Default: generated reads route to the replica pool when configured.
#[autumn_web::repository(Article, table = "articles")]
pub trait ArticleRepository {
    fn find_by_title(title: String) -> Vec<Article>;
}

#[autumn_web::model(table = "ledgers")]
pub struct Ledger {
    #[id]
    pub id: i64,
    pub title: String,
}

// Opt-out: a read-after-write-sensitive aggregate pins all reads to primary.
#[autumn_web::repository(Ledger, table = "ledgers", primary_reads)]
pub trait LedgerRepository {}

// Per-call escape hatch: read-your-writes immediately after a save, without
// dropping to raw Diesel.
#[allow(dead_code)]
async fn read_your_writes(
    repo: PgArticleRepository,
    new: NewArticle,
) -> autumn_web::AutumnResult<Vec<Article>> {
    let saved = repo.save(&new).await?;
    let _on_primary = repo.on_primary().find_by_id(saved.id).await?;
    repo.on_primary().find_all().await
}

fn main() {}
