# DX Audit Report 🗣️

## 1. 🔍 EXPERIENCE - The Walkthrough
I followed the quickstart guide in the `README.md` to the letter:
```bash
cargo install --path autumn-cli
autumn new my-app
cd my-app
cargo run
```
I didn't deviate from the instructions at all, expecting a smooth "works on my machine" experience that ends with visiting `http://localhost:3000`.

## 2. 🚧 STUMBLE - The Friction Points
Instead of a running server, I immediately hit a wall of compilation errors inside the freshly generated `my-app`:
```
error[E0425]: cannot find value `FLASH_CSS_PATH` in module `autumn_web::flash`
error[E0599]: no method named `render` found for struct `autumn_web::flash::Flash` in the current scope
error[E0425]: cannot find function `javascript_include_tag` in this scope
```

**What happened:** The local `autumn-cli` template generates a `src/main.rs` that references new features (`FLASH_CSS_PATH`, `javascript_include_tag`, `flash.render()`). However, the generated `Cargo.toml` specifies `autumn-web = "0.5.0"`, which pulls the published crate from crates.io. The published crate does *not* have these new features yet, so the code simply fails to compile.

## 3. 📢 REPORT - The Complaint
"If I copy-paste the example and it doesn't compile, I am leaving. I just installed the framework exactly as the README told me, created a new app, ran it, and it immediately crashed with a dozen compilation errors about missing macros and paths! I shouldn't have to debug framework internals or deal with version mismatches between the CLI and the core library on minute one. The scaffolding is broken!"

## 4. 🧪 VERIFY - The "idiot proofing"
- Confirmed that running `autumn new my-app` creates a `Cargo.toml` with `autumn-web = "0.5.0"`.
- Confirmed that running `cargo check` inside this generated project fails with the exact errors mentioned above.
- Confirmed that manually overriding `autumn-web` in the generated `Cargo.toml` to point to the local path (`autumn-web = { path = "../autumn", features = ["flash"] }`) fixes the compilation errors, verifying that the issue is a version mismatch between the CLI template and the published crate version.

# DX Audit Report: `autumn new` Scaffolding

## 1. 🔍 EXPERIENCE - The Walkthrough
I followed the quickstart guide in the `README.md` to the letter:
```bash
cargo install --path autumn-cli
autumn new my-app
cd my-app
cargo run
```
I didn't deviate from the instructions at all, expecting a smooth "works on my machine" experience that ends with visiting `http://localhost:3000`.

## 2. 🚧 STUMBLE - The Friction Points
Instead of a running server, I immediately hit a wall of compilation errors inside the freshly generated `my-app`:
```
error[E0425]: cannot find value `FLASH_CSS_PATH` in module `autumn_web::flash`
error[E0599]: no method named `render` found for struct `autumn_web::flash::Flash` in the current scope
error[E0425]: cannot find function `javascript_include_tag` in this scope
```

**What happened:** The local `autumn-cli` template generates a `src/main.rs` that references new features (`FLASH_CSS_PATH`, `javascript_include_tag`, `flash.render()`). However, the generated `Cargo.toml` specifies `autumn-web = "0.5.0"`, which pulls the published crate from crates.io. The published crate does *not* have these new features yet, so the code simply fails to compile.

## 3. 📢 REPORT - The Complaint
"If I copy-paste the example and it doesn't compile, I am leaving. I just installed the framework exactly as the README told me, created a new app, ran it, and it immediately crashed with a dozen compilation errors about missing macros and paths! I shouldn't have to debug framework internals or deal with version mismatches between the CLI and the core library on minute one. The scaffolding is broken!"

## 4. 🧪 VERIFY - The "idiot proofing"
- Confirmed that running `autumn new my-app` creates a `Cargo.toml` with `autumn-web = "0.5.0"`.
- Confirmed that running `cargo check` inside this generated project fails with the exact errors mentioned above.
- Confirmed that manually overriding `autumn-web` in the generated `Cargo.toml` to point to the local path (`autumn-web = { path = "../autumn", features = ["flash"] }`) fixes the compilation errors, verifying that the issue is a version mismatch between the CLI template and the published crate version.
