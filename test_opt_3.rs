use std::collections::HashMap;

struct HandlerRegistry {
    workflows: HashMap<String, ()>,
    activities: HashMap<String, ()>,
    state: Vec<()>,
}

impl std::fmt::Debug for HandlerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerRegistry")
            .field("workflows", &self.workflows.keys())
            .field("activities", &self.activities.keys())
            .field("state_count", &self.state.len())
            .finish()
    }
}

fn main() {
    let mut h = HandlerRegistry {
        workflows: HashMap::new(),
        activities: HashMap::new(),
        state: Vec::new(),
    };
    h.workflows.insert("wf1".to_string(), ());
    println!("{:?}", h);
}
