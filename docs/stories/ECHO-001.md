* 🤦 **The Confusion:**
The `README.md` says that downloading Tailwind CSS is optional:
`# Optional: download Tailwind CSS for styled builds`
`autumn setup`

* 🕵️ **The Reality:**
If I skip `autumn setup` and just run `cargo run`, the generated `build.rs` throws a panic: `Tailwind CSS CLI not found!` and aborts the compilation. It is absolutely required unless I manually delete the `build.rs` file.

* 💡 **The Fix:**
Either change the README to make `autumn setup` a required step, or modify `autumn-cli` to generate a `build.rs` that doesn't panic if Tailwind is missing (e.g., just print a warning and skip building CSS).
