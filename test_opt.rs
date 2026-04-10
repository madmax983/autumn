use std::collections::HashSet;

#[derive(Debug)]
struct WorkflowEvent {
    activity_id: u32,
    name: String,
}

fn test_opt(history: &[WorkflowEvent], activity_name: &str, terminal_ids: &HashSet<u32>) -> Result<u32, String> {
    let mut pending = history
        .iter()
        .filter_map(|event| match event {
            WorkflowEvent {
                activity_id, name, ..
            } if name == activity_name && !terminal_ids.contains(activity_id) => Some(*activity_id),
            _ => None,
        });

    match (pending.next(), pending.next()) {
        (Some(activity_id), None) => Ok(activity_id),
        (None, _) => Err(format!("no pending scheduled activity '{activity_name}' in workflow history")),
        (Some(_), Some(_)) => Err(format!("multiple pending scheduled activities named '{activity_name}' found in history")),
    }
}

fn main() {
    let mut terminal_ids = HashSet::new();
    terminal_ids.insert(2);

    let history = vec![
        WorkflowEvent { activity_id: 1, name: "act1".to_string() },
        WorkflowEvent { activity_id: 2, name: "act1".to_string() },
        WorkflowEvent { activity_id: 3, name: "act2".to_string() },
    ];

    println!("{:?}", test_opt(&history, "act1", &terminal_ids));
    println!("{:?}", test_opt(&history, "act2", &terminal_ids));
    println!("{:?}", test_opt(&history, "act3", &terminal_ids));

    let history_multiple = vec![
        WorkflowEvent { activity_id: 1, name: "act1".to_string() },
        WorkflowEvent { activity_id: 4, name: "act1".to_string() },
    ];
    println!("{:?}", test_opt(&history_multiple, "act1", &terminal_ids));
}
