use autumn_web::prelude::*;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Input;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Output;

#[server]
fn bad_action(_input: Input) -> AutumnResult<Output> {
    todo!()
}

fn main() {}
