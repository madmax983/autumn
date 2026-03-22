# Vendored Dependencies

## htmx

- **Version:** 2.0.4
- **Source:** <https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js>
- **License:** BSD 2-Clause (see <https://github.com/bigskysoftware/htmx/blob/master/LICENSE>)

### How to update

1. Choose the new version at <https://unpkg.com/htmx.org/>.
2. Download the minified build:

   ```sh
   curl -sL https://unpkg.com/htmx.org@<VERSION>/dist/htmx.min.js \
        -o autumn/vendor/htmx.min.js
   ```

3. Update `HTMX_VERSION` in `autumn/src/htmx.rs` to match.
4. Run `cargo test --workspace` to verify the embed still works.
5. Commit both the JS file and the version bump together.
