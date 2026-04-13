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
