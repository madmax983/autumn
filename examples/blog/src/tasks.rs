use autumn_web::config::AutumnConfig;
use autumn_web::prelude::*;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::Deserialize;

use crate::schema::posts;

#[derive(Debug, Default, Deserialize)]
pub struct CleanupPostsArgs {
    #[serde(default)]
    pub confirm: bool,
}

/// Clean up unpublished draft posts.
#[autumn_web::task(name = "cleanup-posts")]
pub async fn cleanup_posts(
    mut db: Db,
    config: AutumnConfig,
    TaskArgs(args): TaskArgs<CleanupPostsArgs>,
) -> AutumnResult<()> {
    let unpublished = posts::table
        .filter(posts::published.eq(false))
        .count()
        .get_result::<i64>(&mut *db)
        .await?;

    if !args.confirm {
        autumn_web::reexports::tracing::info!(
            profile = ?config.profile,
            unpublished,
            "cleanup-posts dry run; pass --confirm to delete unpublished posts"
        );
        return Ok(());
    }

    let deleted = diesel::delete(posts::table.filter(posts::published.eq(false)))
        .execute(&mut *db)
        .await?;

    autumn_web::reexports::tracing::info!(
        profile = ?config.profile,
        deleted,
        "cleanup-posts completed"
    );
    Ok(())
}
