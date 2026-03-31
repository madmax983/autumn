use autumn_web::prelude::*;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct RenameTodo {
    id: i64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct TodoView {
    id: i64,
}

#[server]
async fn rename_todo(input: RenameTodo) -> AutumnResult<TodoView> {
    Ok(TodoView { id: input.id })
}

fn main() {
    let actions = actions![rename_todo];
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].path, "/_autumn/actions/rename_todo");
}
