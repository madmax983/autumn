use autumn_web::prelude::*;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Input<T> {
    value: T,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Output;

#[server]
async fn generic_action<T>(input: Input<T>) -> AutumnResult<Output> {
    let _ = input;
    Ok(Output)
}

fn main() {}
