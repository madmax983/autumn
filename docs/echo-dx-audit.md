# Echo DX Audit Complaint & Fix

## 1. Experience
I followed the README Quickstart:
- Ran `autumn new my-app`
- Ran `cd my-app`
- Ran `cargo run`

## 2. Stumble
I immediately hit three massive friction points:
1. `cargo run` fails because `my-app` thinks it's part of the workspace but it isn't explicitly listed.
2. After fixing that, `cargo run` panics because `Tailwind CSS CLI not found!` in `build.rs`.
3. After fixing that, `cargo run` fails to compile because `Path` is not in scope for `hello_name` (since it uses `0.1.0` from crates.io which didn't have `Path` in the prelude).

## 3. Report
- **"Workspace Error"**: If I generate a new project inside the `autumn` repo (which developers checking out the repo will do), it breaks instantly.
- **"Tailwind Panic"**: `build.rs` shouldn't panic just because an optional CSS tool isn't installed. If Tailwind isn't there, it should just skip CSS compilation with a warning.
- **"Broken Example"**: The generated `main.rs` and the code in `README.md` do not compile out of the box because `Path` is missing from the scope.

## 4. Verify
- Add `[workspace]` to `Cargo.toml.tmpl`.
- Make Tailwind optional in `build.rs.tmpl` (log a warning instead of panicking).
- Use `autumn_web::extract::Path` explicitly in `main.rs.tmpl` and `README.md`.
