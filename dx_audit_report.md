# DX Audit Report 🗣️

## 1. 🔍 EXPERIENCE - The Walkthrough
- Read the `README.md` and followed the quickstart guide.
- Ran `cargo install --path autumn-cli`.
- Ran `autumn new my-app`.
- Copied the example `main.rs` from `README.md` into `my-app/src/main.rs`.
- Attempted to run the app using `cargo run`.
- Visited `http://localhost:3000/hello/echo` and received `Hello, echo!` correctly.

## 2. 🚧 STUMBLE - The Friction Points
- **Error Check 1**: Requesting a missing path like `http://localhost:3000/missing` returns an empty response body (`content-length: 0`), regardless of the `Accept` header (HTML or JSON). Users expect a default 404 page or JSON error object, not a completely blank response.
- **Error Check 2**: Putting a non-existent function inside the `routes!` macro (e.g. `routes![index, missing_route]`) produces a compiler error exposing macro internals: `cannot find function __autumn_route_info_missing_route in this scope`. This makes it harder for users to understand that they simply misspelled a route name.
- **Error Check 3**: Creating an intentional runtime error with duplicate routes results in an Axum panic: `Overlapping method route. Handler for GET / already exists`. While expected for invalid config, a framework-level error catch at startup might be nicer.
- **The "README Run" / Warnings**: During `cargo run`, the console prints warnings about `Tailwind CSS CLI not found`, telling the user to run `autumn setup`. However, the `README.md` explicitly calls `autumn setup` "Optional: download Tailwind CSS for styled builds." If it's optional, it shouldn't produce a constant warning.

## 3. 📢 REPORT - The Complaint
- "Why does a 404 give me an empty page? Simple is better than powerful, but empty is just confusing."
- "If I make a typo in the `routes!` macro, why do I get a weird error about `__autumn_route_info_...`? I am the dumbest person in the room, just tell me 'Route not found'."
- "Why does the CLI yell at me about Tailwind CSS every time I run the app if the README says it's optional?"

## 4. 🧪 VERIFY - The "idiot proofing"
- Confirmed that the `curl -v http://localhost:3000/missing` request responds with HTTP 404 and `content-length: 0` despite having framework error page middleware.
- Confirmed the macro compiler error by intentionally misspelling a route in `routes![]`.
- The `README Run` works as intended, provided you don't make any errors.
# DX Audit Report: `autumn dev` Hot Reloading

## 1. EXPERIENCE

Following the Quickstart guide in `README.md`:

```bash
cargo install --path autumn-cli
autumn new my-app
cd my-app
autumn setup
autumn dev
```

The server started up successfully, watching for file changes.

## 2. STUMBLE

While the default project structure works well, what happens if I want to organize my templates into a separate directory, like `src/views/`?

I created a file `src/views/index.html` and modified it while `autumn dev` was running. However, the server did not detect the change and trigger a rebuild/reload. I expected it to pick up changes in `src/` or common template directories.

## 3. REPORT

The `autumn dev` command currently only watches specific directories for changes: `src`, `static`, `templates`, and `migrations`.

If a developer decides to put their HTML templates in a different directory (e.g., `views`, which is a common convention in web frameworks), `autumn dev` will silently ignore changes to those files. This leads to a frustrating developer experience where the browser doesn't reflect the latest changes, and the developer might assume their code is broken or the dev server has crashed.

## 4. VERIFY

To make the developer experience more robust ("idiot-proofing"):

1.  **Broader Watching:** The watcher should ideally watch the entire project directory (excluding `target/`, `.git/`, etc.) rather than a hardcoded list of directories.
2.  **Configurable Watching:** Alternatively, or additionally, the list of watched directories/files could be configurable in `autumn.toml`.
3.  **Documentation:** If the hardcoded list remains, the documentation must explicitly state *which* directories are watched, so developers know where they can safely put their files and expect hot-reloading to work. Currently, `README.md` says "Development server with file watching" but doesn't specify limitations.


# DX Audit Report: `routes![]` Macro Errors

## 1. 🔍 EXPERIENCE - The Walkthrough
- Attempted to add a new route handler in `routes![]` but misspelled the name.
- Expected a simple 'cannot find function' error for the name I typed.

## 2. 🚧 STUMBLE - The Friction Points
- Got two errors: one for my typo, and a second confusing one: `cannot find function __autumn_route_info_missing_route in this scope`.
- The second error exposes internal macro generation details that I shouldn't have to care about.

## 3. 📢 REPORT - The Complaint
- If I make a typo, just tell me I made a typo. Don't yell at me about `__autumn_route_info_...` which isn't even in my code.

## 4. 🧪 VERIFY - The "idiot proofing"
- Modifying the macro span does NOT remove the second error because rustc will eagerly resolve both. A dummy binding ensures the original user identifier error is surfaced so that developers have clear guidance on what went wrong. We must accept the second macro-level error as unavoidable cost for ergonomic macros.


# DX Audit Report: Application Builder Ergonomics

## 1. 🔍 EXPERIENCE - The Walkthrough
- Read the documentation for `autumn_web::app()`.
- Wrote a basic test application using `.routes()`, `.merge()`, and `.nest()`.
- Intentionally introduced a typo in a route handler name.
- Ran the application using `cargo check`.

## 2. 🚧 STUMBLE - The Friction Points
- **Error Check 1**: Requesting a non-existent route returns an empty HTTP 404 response body (`content-length: 0`).
- **Error Check 2**: Putting a non-existent function inside the `routes!` macro (e.g. `routes![index, missing_route]`) produces a secondary compiler error `cannot find function __autumn_route_info_missing_route in this scope` alongside the primary typo error.
- **Import Scan**: The prelude `use autumn_web::prelude::*;` covers the vast majority of use cases effectively, avoiding "import spam".
- **Slang Check**: Terminology like "routes", "nest", "merge" is standard web framework jargon and easily understandable.

## 3. 📢 REPORT - The Complaint
- "Why does a 404 give me a completely blank page instead of a default error payload?"
- "The macro error `__autumn_route_info_missing_route` exposes internal generation details that I shouldn't have to care about. Just tell me 'Route not found'."

## 4. 🧪 VERIFY - The "idiot proofing"
- Confirmed that standard `curl` requests to missing paths receive `content-length: 0`.
- Verified that the secondary `__autumn_route_info_...` error is unavoidable due to eager macro resolution, meaning we must accept it as an ergonomic trade-off, but it could be explicitly documented.

# 🗣️ Echo: DX Audit for README Run and Route Handlers

## 1. 🔍 EXPERIENCE - The Walkthrough
- Did the "README Run": Copied the exact example code from `README.md` into a new project's `main.rs`.
- Specifically, the example defines `async fn index() -> &'static str { "Welcome to Autumn!" }` and `async fn hello_name(name: autumn_web::extract::Path<String>) -> String`.
- Tested writing a simpler custom route: `async fn foo() -> i32 { 42 }`.

## 2. 🚧 STUMBLE - The Friction Points
- **Error Check**: The `foo` route handler returning `i32` completely fails to compile with a massive, unintelligible error: `the trait bound fn() -> ... {foo}: Handler<_, _> is not satisfied`. The error output references internal Axum routing boundaries: `required by a bound in autumn_web::reexports::axum::routing::get`.
- This tells me that returning plain primitive types like `i32` doesn't work, even though returning strings works.
- **Slang Check**: "Handler trait bound not satisfied" is deep Rust/Axum jargon that breaks the illusion of a simple web framework.

## 3. 📢 REPORT - The Complaint
- "Why can I return a String but not an integer? If I return `42`, the compiler dumps 20 lines of trait bound errors about Axum internals. Simple is better than powerful, and right now simple numbers crash the compiler!"

## 4. 🧪 VERIFY - The "idiot proofing"
- Confirmed that Axum's `IntoResponse` trait is not implemented for `i32`, `i64`, or other plain numbers out-of-the-box, meaning they cannot be returned directly from route handlers without manually converting them to strings or JSON first.

# 🗣️ Echo: [DX Audit] Import Scan (Examples)

## 1. 🔍 EXPERIENCE - The Walkthrough
- Reviewed the `examples/saas/src/routes/` and other examples looking for heavy import requirements.

## 2. 🚧 STUMBLE - The Friction Points
- Discovered that pagination requires deep imports like `use autumn_web::ui::pagination::{PagerOptions, pagination_nav};` or `use autumn_web::pagination::{Page, PageRequest};`
- The `routes![]` macro brings in handlers from various modules, but the framework's own types are not uniformly accessible without multiple `use` statements.

## 3. 📢 REPORT - The Complaint
- "Why do I have to import 12 traits and sub-modules to use simple features like pagination? Simple is better than powerful. If I have to memorize `autumn_web::ui::pagination::pagination_nav`, I'm leaving."

## 4. 🧪 VERIFY - The "idiot proofing"
- Confirmed that adding `pagination_nav` to the `prelude` module or re-exporting it properly would prevent users from having to guess where UI components live.

# 🗣️ Echo: [DX Audit] Slang Check

## 1. 🔍 EXPERIENCE - The Walkthrough
- Looking at the output from `cargo run` and checking out the `actuator/health` endpoint on a fresh app.
- Attempted to read the logs and figure out what the app is doing on startup.

## 2. 🚧 STUMBLE - The Friction Points
- The console logs mention "Centralized trusted-proxy resolution" and "idempotency layers".
- The actuator endpoints mention "actuator" and "probes" which feels heavily Java/Spring Boot inspired.

## 3. 📢 REPORT - The Complaint
- "What on earth is an 'actuator' or 'idempotency layer'? If I'm building a simple web app, I don't want to learn enterprise architecture buzzwords."

## 4. 🧪 VERIFY - The "idiot proofing"
- Checked the `examples/hello` and realized the jargon makes it seem overly complex for beginners.
