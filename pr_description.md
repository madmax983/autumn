🤖 Sentinel: [fix mutants in examples/todo-app and examples/wiki]

🧬 **Mutants Found:**
Found 6 surviving mutants in `todo-app` and 9 surviving mutants in `wiki`.
These were primarily simple rendering functions for templates, like `redirect_to`, `layout`, and `todo_item`, which lacked test coverage and returned default values safely without the tests noticing. There was also a `delete_todo` assertion returning success on `== 0` that allowed partial logic failure to survive.

🎯 **Tests Added/Strengthened:**
* Updated `delete_todo` in `todo-app` to correctly assert exactly 1 row deleted instead of checking for `== 0` (this strengthens logic against partial matching).
* Added unit tests for markup generation functions in `todo-app/src/routes/todos.rs` (`redirect_to`, `index`, `layout`, `todo_item` rendering variations).
* Added unit tests for markup helpers and model updates in `wiki/src/routes/mod.rs` inside a `tests` module. Added tests for `layout`, `status_badge`, `redirect_to`, `new_form`, and `into_update`.

⚠️ **Suspected Bugs:**
`[Suspected Code Bug]` The `delete_todo` in `todo-app` originally checked `if deleted == 0 { return Err(AutumnError::not_found(...)) }`. A tighter assert `if deleted != 1` was added to verify exactly one item is removed.

📊 **Kill Rate:**
Reduced `todo-app` survivors from 6 to 1 (the remaining one is `main`).
Reduced `wiki` survivors from 9 to 1 (again, `main`).

🔗 **Havoc Interaction:**
No direct Havoc overlap, as these issues were mostly regarding HTML string assertion correctness and rendering configurations, not deep concurrency edge cases.
