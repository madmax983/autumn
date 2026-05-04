use autumn_web::prelude::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CleanupArgs {
    user_id: i64,
    dry_run: bool,
}

/// Clean up stale rows for a single user.
#[autumn_web::task(name = "cleanup-user")]
async fn cleanup_user(
    State(_state): State<AppState>,
    TaskArgs(args): TaskArgs<CleanupArgs>,
) -> AutumnResult<()> {
    assert!(args.user_id > 0);
    let _ = args.dry_run;
    Ok(())
}

fn main() {
    let tasks = one_off_tasks![cleanup_user];
    assert_eq!(tasks[0].name, "cleanup-user");
    assert_eq!(tasks[0].description, "Clean up stale rows for a single user.");
}
