import re

with open("autumn-harvest/autumn-harvest/src/worker.rs", "r") as f:
    content = f.read()

# Fix 1: HandlerRegistry
content = content.replace('.field("workflows", &self.workflows.keys().collect::<Vec<_>>())', '.field("workflows", &self.workflows.keys())')
content = content.replace('.field("activities", &self.activities.keys().collect::<Vec<_>>())', '.field("activities", &self.activities.keys())')

# Fix 2: find_pending_activity
search_block = """    let pending = history
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::ActivityScheduled {
                activity_id, name, ..
            } if name == activity_name && !terminal_ids.contains(activity_id) => Some(*activity_id),
            _ => None,
        })
        .collect::<Vec<_>>();

    match pending.as_slice() {
        [activity_id] => Ok(*activity_id),
        [] => Err(HarvestError::NotFound(format!(
            "no pending scheduled activity '{activity_name}' in workflow history"
        ))),
        _ => Err(HarvestError::NonDeterministic(format!(
            "multiple pending scheduled activities named '{activity_name}' found in history"
        ))),
    }"""

replace_block = """    let mut pending = history
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent::ActivityScheduled {
                activity_id, name, ..
            } if name == activity_name && !terminal_ids.contains(activity_id) => Some(*activity_id),
            _ => None,
        });

    match (pending.next(), pending.next()) {
        (Some(activity_id), None) => Ok(activity_id),
        (None, _) => Err(HarvestError::NotFound(format!(
            "no pending scheduled activity '{activity_name}' in workflow history"
        ))),
        (Some(_), Some(_)) => Err(HarvestError::NonDeterministic(format!(
            "multiple pending scheduled activities named '{activity_name}' found in history"
        ))),
    }"""

content = content.replace(search_block, replace_block)

with open("autumn-harvest/autumn-harvest/src/worker.rs", "w") as f:
    f.write(content)
