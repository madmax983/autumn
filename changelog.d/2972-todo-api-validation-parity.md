### Fixed

- **todo-app:** the JSON API (`POST /api/todos`) now enforces the model's
  own derived validation rules — including `length(max = 255)` — the same
  rules the HTML form path already enforced, returning 422 with field-level
  details on violation (issue #2972). Previously the hand-rolled
  `NewTodo::validated()` only trimmed and rejected empty titles, so the
  JSON API (and its MCP projection) silently accepted overlong titles.
