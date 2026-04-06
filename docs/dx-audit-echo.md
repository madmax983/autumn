# Echo: DX Audit Complaint & Fix

## 1. Experience
I followed the "README Run" by literally copy-pasting the "Quickstart" example code into a fresh `main.rs` file.

After setting up a basic app, I wanted to try and read the query string from the path since that's a very common thing. I intuitively tried:

```rust
use autumn_web::prelude::*;

#[get("/hello")]
async fn hello_query(query: Query<std::collections::HashMap<String, String>>) -> String {
    format!("Query: {:?}", *query)
}
```

## 2. Stumble
The example code failed to compile!

```
error[E0425]: cannot find type `Query` in this scope
 --> src/main.rs:4:29
  |
4 | async fn hello_query(query: Query<std::collections::HashMap<String, String>>) -> String {
  |                             ^^^^^ not found in this scope
```

## 3. Report
The `Query` extractor is a core web concept. The documentation promises that frequently used Axum extractors should be re-exported in `autumn_web::prelude::*` to provide a seamless, low-boilerplate developer experience. However, not having `Query` in the prelude forces me to read the source code and guess its import path or import it manually, breaking the low-boilerplate DX promise. "Simple" is better than "Powerful", and having to hunt for imports is not simple.

## 4. Verify
Export `Query` from `autumn_web::extract` and re-export it in `autumn_web::prelude`. I checked the source code, and indeed `Path`, `Form`, and `Json` are present but `Query` is missing.
