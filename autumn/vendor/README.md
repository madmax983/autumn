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

## Swagger UI

- **Version:** 5.32.4
- **Source:** <https://unpkg.com/swagger-ui-dist@5.32.4/swagger-ui.css>
- **Source:** <https://unpkg.com/swagger-ui-dist@5.32.4/swagger-ui-bundle.js>
- **License:** Apache-2.0 (see <https://github.com/swagger-api/swagger-ui/blob/master/LICENSE>)

### How to update

1. Download the pinned distribution assets:

   ```sh
   curl -sL https://unpkg.com/swagger-ui-dist@<VERSION>/swagger-ui.css \
        -o autumn/vendor/swagger-ui/swagger-ui.css
   curl -sL https://unpkg.com/swagger-ui-dist@<VERSION>/swagger-ui-bundle.js \
        -o autumn/vendor/swagger-ui/swagger-ui-bundle.js
   ```

2. Update `SWAGGER_UI_VERSION` in `autumn/src/openapi.rs` to match.
3. Run the OpenAPI and security tests to verify the same-origin UI still works under the default CSP.
