# 🗣️ Echo: Developer Experience Audit Findings

## Missing Path Extractor in Prelude

* 🤦 **The Confusion:** The `README.md` and `examples/hello` use `autumn_web::extract::Path<String>`. I had to type out a 4-level deep import just to read a path parameter. That's way too much typing and it looks ugly in simple examples.
* 🕵️ **The Reality:** The `autumn_web::prelude` module exports `Json` and `Form`, but completely omits `Path` and `Query`.
* 💡 **The Fix:** Add `Path` and `Query` to `autumn_web::prelude`.

## Horrible Error Messages for Invalid Handlers

* 🤦 **The Confusion:** I made a mistake and forgot `Path<>` in my route handler parameter (`async fn hello_name(name: i32)`). I got a 50-line error from Axum about `Handler<_, _>`. The compiler told me to add `#[axum::debug_handler]` to improve the error message, but when I added it, it said `axum` is undeclared because it's not in my `Cargo.toml`.
* 🕵️ **The Reality:** The framework removes `debug_handler` to avoid missing path errors, but leaves the user with terrible type-bound errors and suggestions to use a tool they can't access.
* 💡 **The Fix:** Find a way to restore `debug_handler` (maybe a wrapper like `#[autumn_web::debug_handler]`) or provide a framework-specific equivalent so we get good errors back.

## Jargon in Documentation

* 🤦 **The Confusion:** The README mentions "Hybrid rendering" and "Escape hatches". I don't know what these mean. Is this a car engine? Am I escaping a submarine?
* 🕵️ **The Reality:** The terms are slang or heavy framework jargon.
* 💡 **The Fix:** Use simpler language or explain the terms. "Pre-rendering pages to static HTML" is better than "Hybrid rendering", and "customization options" is better than "escape hatches".
