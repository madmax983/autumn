use autumn_harvest::prelude::*;

#[workflow]
async fn test_workflow(ctx: &WorkflowContext, _input: String) -> Result<String, String> {
    let _ = ctx;
    Ok("done".into())
}

#[test]
fn workflow_companion_exists_and_returns_info() {
    let info = __autumn_workflow_info_test_workflow();
    assert_eq!(info.name, "test_workflow");
}
