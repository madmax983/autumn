use autumn_web::prelude::*;
use autumn_web::task::TaskCoordination;

#[scheduled(every = "10s", name = "cache-warm", coordination = "per_replica")]
async fn cache_warm(_state: AppState) -> AutumnResult<()> {
    Ok(())
}

#[scheduled(every = "10s", name = "fleet-cleanup")]
async fn fleet_cleanup(_state: AppState) -> AutumnResult<()> {
    Ok(())
}

fn main() {
    let tasks = tasks![cache_warm, fleet_cleanup];

    assert_eq!(tasks[0].coordination, TaskCoordination::PerReplica);
    assert_eq!(tasks[1].coordination, TaskCoordination::Fleet);
}
