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
