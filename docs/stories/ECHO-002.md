# 🗣️ Echo: Developer Experience Audit Findings

## Mandatory Setup Hidden as "Optional"

* 🤦 **The Confusion:** The `README.md` explicitly says `autumn setup` is "Optional: download Tailwind CSS for styled builds", but when I skip it and run `cargo run`, the build immediately crashes and panics with `Tailwind CSS CLI not found!`.
* 🕵️ **The Reality:** The generated `build.rs` requires the `tailwindcss` CLI to compile the project, and it crashes the entire build process via `panic!` if the executable isn't found. This makes the step absolutely mandatory, not optional.
* 💡 **The Fix:** Either explicitly document `autumn setup` as a required step in the README, or modify the generated `build.rs` to emit a warning instead of a panic if Tailwind isn't installed.

## Workspace Conflict on `autumn new`

* 🤦 **The Confusion:** I ran `autumn new my-app`, then `cd my-app` and `cargo run` just like the README said. But the compiler yelled at me: `current package believes it's in a workspace when it's not`. I didn't even know what a workspace was.
* 🕵️ **The Reality:** If a user runs `autumn new` inside an existing Git repository or an upper-level directory containing a `Cargo.toml` with a `[workspace]`, the newly generated project gets absorbed into it, breaking the build because it's not explicitly declared as a member.
* 💡 **The Fix:** Add an empty `[workspace]` block at the bottom of the generated `Cargo.toml` in `autumn new` to isolate the new project and guarantee it compiles anywhere.

## The Import Scan: `autumn_web::extract::Path`

* 🤦 **The Confusion:** The `README.md` Quickstart example requires me to write `name: autumn_web::extract::Path<String>` just to get a string from a URL. That's a massive, 4-level deep import just to read "hello/john".
* 🕵️ **The Reality:** While `autumn_web` is intended to be simple, essential types like `Path` are hidden deep in `extract` and missing from a top-level module or prelude.
* 💡 **The Fix:** Re-export common extractors like `Path` and `Query` in the root or a `prelude` module so I can just write `name: Path<String>`.
