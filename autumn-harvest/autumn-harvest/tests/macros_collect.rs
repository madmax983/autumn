use autumn_harvest::prelude::*;
use autumn_harvest_macros::{activities, workflows};

#[workflow]
async fn wf_a(ctx: &WorkflowContext, _x: String) -> Result<(), String> {
    let _ = ctx;
    Ok(())
}

#[workflow]
async fn wf_b(ctx: &WorkflowContext, _x: String) -> Result<(), String> {
    let _ = ctx;
    Ok(())
}

#[activity]
async fn act_a(ctx: &ActivityContext, _x: String) -> Result<(), String> {
    let _ = ctx;
    Ok(())
}

#[test]
fn workflows_macro_collects_correct_count() {
    let wfs: Vec<WorkflowInfo> = workflows![wf_a, wf_b];
    assert_eq!(wfs.len(), 2);
    assert_eq!(wfs[0].name, "wf_a");
    assert_eq!(wfs[1].name, "wf_b");
}

#[test]
fn activities_macro_collects_correct_count() {
    let acts: Vec<ActivityInfo> = activities![act_a];
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0].name, "act_a");
}
