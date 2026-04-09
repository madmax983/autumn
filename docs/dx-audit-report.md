# Echo DX Audit Complaint & Fix

## 1. 🔍 EXPERIENCE - The Walkthrough
Followed the "README Run" by executing `autumn new my-app` and then running `cargo build` inside the new project.

## 2. 🚧 STUMBLE - The Friction Points
1. **Tailwind Panic**: The build panicked immediately because `Tailwind CSS CLI not found!` in `build.rs`. New users who just want to see the "Hello World" app run are punished with a build script panic instead of a graceful degradation.
2. **Missing Prelude Import**: When trying to use standard Axum extractors like `Query`, the compiler complains about missing imports because `Query` is not re-exported in `autumn_web::prelude::*`, forcing the user to dig into framework internals.

## 3. 📢 REPORT - The Complaint
- "Panic in `build.rs` is hostile to new users. If I copy-paste the example and it doesn't compile, I am leaving."
- "I shouldn't have to manually import `autumn_web::extract::Query` when `Path`, `Json`, and `Form` are already in the prelude. Keep it simple."

## 4. 🧪 VERIFY - The "idiot proofing"
- Modify `build.rs.tmpl` to treat the Tailwind step as optional. If the CLI is missing, simply emit a cargo warning and continue the build gracefully.
- Add `pub use crate::extract::Query;` to `autumn/src/prelude.rs` so all standard extractors are consistently available out-of-the-box.
