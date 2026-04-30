# Changelog

All notable changes to the Autumn framework will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **authorization:** First-class record-level authorization — `Policy` /
  `Scope` traits, `PolicyContext` carrying the resolved `Session` /
  user / role set / `Db` handle / `PolicyRegistry`, an `#[authorize("action",
  resource = Type)]` attribute macro that resolves the registered policy
  and short-circuits with the configured deny response before the
  handler body runs, a `policy = SomePolicy` argument on
  `#[repository(api = "...")]` that wires the same checks into every
  auto-generated POST/PUT/DELETE endpoint plus `GET /<api>/{id}` for
  read scoping, a `scope = SomeScope` companion that constrains list
  endpoints, a `Scoped` blanket trait that adds
  `Post::scope(&ctx).load(&mut db).await?` ergonomics to every type
  (mirroring Pundit's `policy_scope`), and a `[security]
  forbidden_response = "404" | "403"` knob (default `"404"` to mirror
  Rails / Phoenix and avoid leaking record existence). Register on the
  app builder via `.policy::<R, _>(...)` / `.scope::<R, _>(...)`.
  `examples/reddit-clone`'s `PostPolicy` replaces every hand-rolled
  `if post.author_id != user_id` check; `git grep -n "author_id != user_id"
  examples/reddit-clone/` now returns empty (#496).
  - **Behavior change:** `#[repository(api = "...")]` without a paired
    `policy = ...` argument is now a startup-time error in `prod`
    profile builds. The escape hatch is `[security]
    allow_unauthorized_repository_api = true`. Downstream apps using the
    `api =` switch must add `policy = SomePolicy` (or opt out
    explicitly) — this is the one behavior change downstream apps have
    to address.
- **storage:** New optional `autumn-web` `storage` cargo feature (off by default) introducing a pluggable file-storage abstraction (#494). Adds the `BlobStore` trait (`put`, `get`, `delete`, `head`, `presigned_url`, `put_stream`), the `Blob` value type with Postgres `JSONB` round-tripping via Diesel `AsExpression` / `FromSqlRow`, a `Local` backend with HMAC-signed URLs and an autumn-mounted serving route at `[storage.local].mount_path` (default `/_blobs`), the `add_blob_column!` migration helper macro, and a feature-gated `S3BlobStore` shell behind `storage-s3` (real SDK wiring tracked as #530). `MultipartField::save_to_blob_store` streams uploads straight through to `BlobStore::put_stream` without buffering, with multipart parser errors preserving their client-facing 4xx status. Profile-aware defaults mirror sessions: `dev` auto-defaults to `Local` rooted at `target/blobs/`; `prod` fails fast on `local` unless `storage.allow_local_in_production = true` is explicitly set. The local backend uses temp-file + cross-platform atomic-replace semantics so partial writes never leave corrupted blobs and re-uploads work on Windows as well as POSIX. `examples/reddit-clone` carries the database-backed UI demo (`Blob` column on the user model + upload form + presigned-URL profile picture); framework-level integration tests pin restart survival on the `Local` backend. Apps that don't enable `storage` see no surface change — this is non-breaking. See [`docs/guide/storage.md`](docs/guide/storage.md).
- **cli:** `autumn generate model | migration | scaffold` for one-command
  resource scaffolding (#493). Emits `#[model]` structs, Diesel migrations,
  `schema.rs` entries, `#[repository(api = ...)]` blocks, Maud HTML route
  handlers, smoke tests, and updates `routes![]` in `src/main.rs`. Supports
  the documented field-type DSL (`String`, `Text`, `i32`, `i64`, `bool`,
  `f32`, `f64`, `Uuid`, `NaiveDateTime`, `DateTime`, `Vec<u8>`/`Bytea`, plus
  `Option<…>` for any of them) and `--dry-run` / `--force` flags. New
  `docs/guide/generators.md` walks through the five-commands-to-CRUD flow.
  Non-breaking; no runtime surface changes.
- **path-helpers:** Typed URL path helpers generated from route macros (#499).
  Every `#[get]`, `#[post]`, `#[put]`, `#[delete]`, and `#[patch]` macro now
  emits a `pub fn __autumn_path_{name}(params...) -> PathBuilder` companion
  alongside the route handler. The `paths![]` collection macro gathers these
  into a `pub mod paths { ... }` block so callers write
  `paths::show_post(post.id)` instead of `format!("/posts/{}", post.id)`.
  Path parameter types are inferred from the handler's `Path<T>` extractor
  (single type or tuple for multiple params); untyped params fall back to
  `impl Display`. `PathBuilder` supports fluent query-string construction via
  `.with_query("key", value)` with RFC 3986–compliant percent-encoding, and
  `.into_redirect()` for zero-boilerplate `axum::response::Redirect` returns.
  An optional `#[name = "custom_name"]` attribute on any route handler
  overrides the generated helper name to avoid collisions across modules or
  improve call-site readability. Non-breaking: existing `format!("/…", …)`
  call sites continue to compile; helpers are purely additive.
  See [`docs/guide/path-helpers.md`](docs/guide/path-helpers.md).

## [0.3.0] - 2026-04-27

### Added

- Add autumn-admin-plugin with auto-generated CRUD UI (#455)([4486405](https://github.com/madmax983/autumn/commit/44864052036aba83740b664595cbbef1f93bdfa2))
- **audit:** Add first-class structured audit logging API (#437)([4ac0f7c](https://github.com/madmax983/autumn/commit/4ac0f7c92b3271616175f80216c8fa4b535dca13))
- Add hx_location support to HxResponseExt (#408)([0f6ea9d](https://github.com/madmax983/autumn/commit/0f6ea9db578b6ed5ac54072b86dca458d90fa4f6))
- **security:** Htmx-friendly default CSP for secure-headers (S-049)([a71f1af](https://github.com/madmax983/autumn/commit/a71f1af905ea6f7aaf53c3201912960775b2d94e))
- **security:** Built-in per-IP rate limiting (S-047)([68ccada](https://github.com/madmax983/autumn/commit/68ccadab4d68757d9c924e8250fd96b783ac9159))
- **security:** CSRF error body + route-specific exempt_paths (S-046)([8ecc78e](https://github.com/madmax983/autumn/commit/8ecc78ea9cf7ba27a22ac75e6a2f81e52a83bd64))
- **app:** Complete raw axum route mounting coverage and docs([55ae63a](https://github.com/madmax983/autumn/commit/55ae63a0e07b8a5540970e818564716a8cbf0f9e))
- **app:** Add AppBuilder::layer for custom Tower middleware (S-049)([62c33a2](https://github.com/madmax983/autumn/commit/62c33a2ef0a601c0129babda2943ba21e63c89f9))
- Trait-based subsystem replacement for config / DB / telemetry / session (S-053)([89683ed](https://github.com/madmax983/autumn/commit/89683edbf261f7ce580efd00fea4389b5c4556e3))

### Fixed

- Patched MSRV([f56a82d](https://github.com/madmax983/autumn/commit/f56a82de71d57c9bee09db6a1862140535d67cfb))
- Crate version issue([54fcd7b](https://github.com/madmax983/autumn/commit/54fcd7bfa31e9a1c54c7a9eafdbfeac3da8c7c1a))
- Vendor swagger-ui([6cbac95](https://github.com/madmax983/autumn/commit/6cbac95187ded4c20abea257582163ccd02de8b1))
- Multipart_rejection_to_error([516dbc5](https://github.com/madmax983/autumn/commit/516dbc5eedf2188ce8e5bf32dea41f9a83f1875e))
- Resolve intra-doc link warnings in cargo doc (#450)([38b743d](https://github.com/madmax983/autumn/commit/38b743d715b17e3e4463a2ba47e5319a8ecb4b1c))
- **dev:** Serve live-reload script from /__autumn/live-reload.js([1e82da0](https://github.com/madmax983/autumn/commit/1e82da0e7d2438841764b8b01938eabbc4283bda))
- Resolve clippy linting errors in error_page_filter.rs([b3ca678](https://github.com/madmax983/autumn/commit/b3ca678d1d62d4812200ee32e90c6cee8a864175))
- **rate-limit:** Bypass when no identifiable client (P1)([475a6e6](https://github.com/madmax983/autumn/commit/475a6e6c05f04fc9ab9041c45946779026ca848b))
- **rate-limit:** Untrust forwarding headers by default; fix sweep([784e9a7](https://github.com/madmax983/autumn/commit/784e9a705a80af060079a926b4991be644cb5678))
- **cors:** Reject wildcard+credentials, warn on malformed values (S-048)([0477f49](https://github.com/madmax983/autumn/commit/0477f49092573721bf70895600f7a12fdca9edbf))
- **S-049:** Review polish + apply custom layers in static build mode([fa0f1d0](https://github.com/madmax983/autumn/commit/fa0f1d05193f66962285db3e444b4a226ddc87b1))
- Eliminate panic risks in config merge and test telemetry fallback([50ad773](https://github.com/madmax983/autumn/commit/50ad7730f90a39d1f4e6f5d85818b0a976192713))
- Restore fail-fast session validation for the default session path([069c509](https://github.com/madmax983/autumn/commit/069c5099f3ed7688bba50cb902a418fac9293c51))
- Bypass session config validation when custom store is configured([d3b4fad](https://github.com/madmax983/autumn/commit/d3b4fade99f4e2a4048a2040f1da151d45871980))
- Address Codex review on PR #382 (P1 + P2)([7991aa3](https://github.com/madmax983/autumn/commit/7991aa3c179c02d8b9425c684e600216c5c65465))
- Expose telemetry module + TelemetryGuard::disabled() publicly([81711fc](https://github.com/madmax983/autumn/commit/81711fc1deb6d059aaf8c4e937d57ef3cab4a113))

### Performance

- **config:** Optimize levenshtein to use a single vector (#419)([896611f](https://github.com/madmax983/autumn/commit/896611fd5db14214b124452fe6739c3026b20a58))
- **rate_limit:** Use zero-cost numeric HeaderValue conversion (#405)([2edf7fe](https://github.com/madmax983/autumn/commit/2edf7fe4f0e85361d1b7f1c379bf164b6eebb6bf))

### Changed

- Implement Display for Schedule and simplify formatting (#418)([d90bb68](https://github.com/madmax983/autumn/commit/d90bb68653415d0a0194730a428cc6dbf8790023))
- **app:** Sealed IntoAppLayer trait for readable compile errors([ea5dd83](https://github.com/madmax983/autumn/commit/ea5dd833ecbbc285b9f76de9b3c5c5b810f760ab))
- **config:** Extract parse_env_option_string helper([13f97f9](https://github.com/madmax983/autumn/commit/13f97f9e2d5ca8c82c008b70c6711b442bbe2648))
- **config:** Extract parse_env_option_string helper([36650dd](https://github.com/madmax983/autumn/commit/36650dd279e9badd2d97da311dd9d6e6dc0a9b70))

### Documentation

- Skill([30b5e21](https://github.com/madmax983/autumn/commit/30b5e21798f5d9403f5fea2d95f5d126e24ebf0c))
- Fix broken rustdoc intra-doc links (#475)([99e1e2d](https://github.com/madmax983/autumn/commit/99e1e2d6e5fbb8fbeddadc623597b8265ca99c54))
- Add SemVer stability policy and MSRV-alignment CI check (#433)([54692c9](https://github.com/madmax983/autumn/commit/54692c9fc8f4deb669b8c5871fa02017db8b0201))
- Add Vantage spec for configurable dev watcher (#422)([0abb47e](https://github.com/madmax983/autumn/commit/0abb47ee4abf9a9fe05baaaa556dbce23d1d74f4))
- Append DX audit report for primitive return type compilation errors (#421)([fa30f20](https://github.com/madmax983/autumn/commit/fa30f20629ad8bb1a9804a4d9bb651e275e9250a))
- Verify tests for __check_secured (#417)([c71ccee](https://github.com/madmax983/autumn/commit/c71ccee82ef5b77542bbe8ed530ec82680db86d1))
- Add Vantage spec for middleware introspection (#409)([1b3d260](https://github.com/madmax983/autumn/commit/1b3d26067dea7cd96322d196eff5c81115d46b78))
- Drop stale status block from README (#379)([eef956e](https://github.com/madmax983/autumn/commit/eef956ea11f6715fd83e8c721d62962ab1e226b8))
- Update CHANGELOG.md for v0.2.0([7b4d922](https://github.com/madmax983/autumn/commit/7b4d922aa2abf01d8aa55a483032434b2f70b6ed))

### Styling

- Rustfmt merge resolution([9831b22](https://github.com/madmax983/autumn/commit/9831b2228c4b9181be6cf6ba4780f5c4c72e928b))
- Rustfmt([86e9b4c](https://github.com/madmax983/autumn/commit/86e9b4c5e9d8034317267cdd46a1b9da71cb2e83))
- **security:** Rustfmt fix for CSRF error response([b30d69d](https://github.com/madmax983/autumn/commit/b30d69d232392c3cb842d65664f7fefab29bceab))
- Apply rustfmt to preflight test([d61fff4](https://github.com/madmax983/autumn/commit/d61fff459d2a12bee438c54f29c5f0612463c453))

### Testing

- Add test coverage for HEAD requests in fallback_404_handler (#485)([d5a2da8](https://github.com/madmax983/autumn/commit/d5a2da8722cfe203695b8fa2227724fc3a2beac1))
- Add test coverage for pagination mutants (#469)([637fb83](https://github.com/madmax983/autumn/commit/637fb8318641401938b9a0e82f34ddbe6790955b))
- **flash:** Strengthen flash module tests to kill surviving mutants (#430)([2fb18c5](https://github.com/madmax983/autumn/commit/2fb18c54b00e0b235317075cad7d6db55a64f525))
- Add test coverage for hash_password (#416)([94adba4](https://github.com/madmax983/autumn/commit/94adba4c824e7897e43d294d5f902a5934b817bc))
- Acknowledge existing coverage for fallback_404_handler (#415)([7bfea47](https://github.com/madmax983/autumn/commit/7bfea4794051987ffb16535aaa15fe31b2f89615))
- Add coverage for init_with_telemetry (#413)([378f17c](https://github.com/madmax983/autumn/commit/378f17cd4aebf3d0247552c1e588dd0df3d1f417))
- Add test for live_reload_state_handler (#411)([7f3e64c](https://github.com/madmax983/autumn/commit/7f3e64cfabfa2d912dfe4c7fc78fd8ecfc9f968f))
- Close mutant gap in DieselDeadpoolPoolProvider::create_pool (#406)([759c5ee](https://github.com/madmax983/autumn/commit/759c5eeb4b928f5cb6cb6dd4161d4e85bef8b772))

### Miscellaneous

- Version tags([3d5c171](https://github.com/madmax983/autumn/commit/3d5c171e5f5d738cb89af87b12112a9ce62637f5))
- Version tagging([8c62662](https://github.com/madmax983/autumn/commit/8c626629e6939f95dc242e592cdc8ff17c23ebb7))
- PR feedback([86ebfd8](https://github.com/madmax983/autumn/commit/86ebfd8db6153f76592fd927e2b6c3354808d379))
- Cleanup([169c894](https://github.com/madmax983/autumn/commit/169c894b37e430fdbe06bc30dbb157288f0d01cf))
- Trigger on trunk-dev push and pull_request (#376)([8a46d2c](https://github.com/madmax983/autumn/commit/8a46d2c0aa748513c1f7d01a25774c1b3c6a500b))
- Fmt([660cf10](https://github.com/madmax983/autumn/commit/660cf10f3c78b0187b1aa02613a75c8e1dd1cb51))
- Use RwLock instead of Mutex for AppState extensions (#370)([f47e46d](https://github.com/madmax983/autumn/commit/f47e46d2a068f3daac9e8a615df2c2a0c178b263))

### Refactor

- Re-export axum::extract::State to hide axum dependency([d35ccc5](https://github.com/madmax983/autumn/commit/d35ccc50c32f44f811b18a9427d88c9160c0cc5c))
- Re-export axum::extract::State to hide axum dependency([407c4ca](https://github.com/madmax983/autumn/commit/407c4cae415cbe2b19b2d6c8ead0723ccbaab442))

### Merge

- Resolve conflicts with trunk-dev (rate-limit + CSP features)([5b0397d](https://github.com/madmax983/autumn/commit/5b0397d99267822cacc3e27f973135e554c35897))

### Sentry

- Eliminate unchecked unwraps (#445)([79c7caf](https://github.com/madmax983/autumn/commit/79c7caf774294edbdb246e4058afd2dbf9fda21b))
## [0.2.0] - 2026-04-19

### Added

- Bridge Channels pubsub with SSE streams for htmx (#344)([8497afd](https://github.com/madmax983/autumn/commit/8497afda4257077ef0a3ce41df025646f02b3c89))
- Add HxResponseExt trait for fluid HTMX response header configuration (#274)([fbe8630](https://github.com/madmax983/autumn/commit/fbe8630abff0f4da30ff85abac4651eb610be8f5))
- Add harvest topology escape hatches (#223)([e55a1be](https://github.com/madmax983/autumn/commit/e55a1be80dd9186fe175f488aff5188842c154b0))
- **actuator:** Add prometheus metrics exporter (#164)([351d3da](https://github.com/madmax983/autumn/commit/351d3daed0830e1fb465c747a64899c0b6d81f5a))
- **error:** Add 500 error constructors to AutumnError (#157)([02396e9](https://github.com/madmax983/autumn/commit/02396e9e9bb5f2210590c28d3cb2fc53f82c9182))
- **harvest:** Implement Phase 5 signal delivery and query registry (#113)([c4ab5b8](https://github.com/madmax983/autumn/commit/c4ab5b8db2b0a25cb41488c129c57c5495a82ff8))
- **harvest:** Add replay-aware child workflow command support (#98)([58c0bb3](https://github.com/madmax983/autumn/commit/58c0bb311b90bef8f2808a90f812319342f6a616))
- Add autumn-harvest durable workflow engine (#57)([aa10042](https://github.com/madmax983/autumn/commit/aa10042cb95cdda57b175394fa211460e340a688))
- Implement autumn-harvest Phase 1 — durable workflow engine foundation (#43)([819e993](https://github.com/madmax983/autumn/commit/819e9931e32e9982d5615134613dd080cf3c9564))
- Add v0.2 features — actuator endpoints, migrations, error pages, hybrid rendering Phase 2, raw Axum escape hatch (#37)([df31508](https://github.com/madmax983/autumn/commit/df315085c4adc4fb0720389e817e9a7ad6cd34f3))
- **macros:** Add #[service] macro for cross-model orchestration (#36)([114f292](https://github.com/madmax983/autumn/commit/114f29246f031fab85770593ec7101415d491758))
- **wiki:** Add REST API via api macro([fefbcf6](https://github.com/madmax983/autumn/commit/fefbcf6304044f5223ed31db6fc695601edfa34a))
- **macros:** Generate CRUD API handlers from api = "/path"([a13971b](https://github.com/madmax983/autumn/commit/a13971bfe21aed8304859fbb61194fec49d2d21b))
- **macros:** Parse api = "/path" in #[repository] attribute([8e701e9](https://github.com/madmax983/autumn/commit/8e701e972d9499f50955051a1838dac32c60f47e))
- Hooks integration, wiki example, and i64 migration (#29)([017f2ce](https://github.com/madmax983/autumn/commit/017f2cef78d7989633cbae193e21627c8c7c2b12))
- **hooks:** Add UpdateDraft<T> and DraftField<'a, T> types (#28)([0b853f2](https://github.com/madmax983/autumn/commit/0b853f222cede82fb721fd50a0a82182682d6108))
- Hybrid rendering Phase 1 — #[static_get] macro and StaticFileLayer (#25)([f2b62dc](https://github.com/madmax983/autumn/commit/f2b62dc9ca19c4fc374f9a42ec8c7f9a2b64dd50))
- Add bookmarks example showcasing v0.2 features([3fe79f0](https://github.com/madmax983/autumn/commit/3fe79f0719efb26144913c4b6beeaf9afb443d14))
- Add blog engine example([f52eb1f](https://github.com/madmax983/autumn/commit/f52eb1f468517a796c63196eb79e6b552ad4bf07))

### Fixed

- **session:** Prevent cookie tossing vulnerability in session cookie extraction (#286)([5c854ca](https://github.com/madmax983/autumn/commit/5c854ca1e47894da2e5566fc4ab0a8e6207135e3))
- Handle integer overflow gracefully in parse_duration (#236)([c99ad94](https://github.com/madmax983/autumn/commit/c99ad94cca2ed3da930eaeae9ee11a834d7f77c9))
- **cli:** Handle missing tailwind cli gracefully in build.rs template (#226)([fc85378](https://github.com/madmax983/autumn/commit/fc85378cb81e5123f56a233a40109ee9a27ecb76))
- Harden harvest listen notify sql (#174)([8ff0359](https://github.com/madmax983/autumn/commit/8ff0359294b61a38f89a631b16a322d0747a1ee1))
- Re-export Path extractor in prelude for better DX (#124)([076f574](https://github.com/madmax983/autumn/commit/076f5749f9c55e18f5e77f3db56ccab7ae324745))
- **wiki:** Use PageForm for create route to avoid missing slug field([e644b28](https://github.com/madmax983/autumn/commit/e644b28d06581cad9d874c4489e422a5e14aa580))
- Bookmarks example CSS, form submission, and missing files (#24)([6528ca7](https://github.com/madmax983/autumn/commit/6528ca7fb9b49c400e70953398b9dc2a64313885))
- Resolve #[repository] macro path issues for downstream crates (#23)([616855b](https://github.com/madmax983/autumn/commit/616855b1f0c302dc39766a01fe93e78a8ea16440))
- Update trybuild expected error for #[model] on enum([347e868](https://github.com/madmax983/autumn/commit/347e86879f6b1155f522701554fed7a550200c9b))
- Resolve CI lint errors (needless raw string hash, unused import)([401b12b](https://github.com/madmax983/autumn/commit/401b12bdc60691e8b4f6d64228ade3cfd4ffe0fc))
- Add version requirement to autumn-macros dep for crates.io publish([6216345](https://github.com/madmax983/autumn/commit/6216345e0ad9de6f1c2ea0db477dab1744672b69))

### Performance

- Optimize levenshtein to avoid intermediate string allocations (#131)([6dfc1f4](https://github.com/madmax983/autumn/commit/6dfc1f4ee8080e8bff501efeab2da1d4d07a9caf))
- **metrics:** Optimize compute_percentiles to O(N) using select_nth_unstable (#95)([470a0b4](https://github.com/madmax983/autumn/commit/470a0b41fb5317b204e3f491fe4cf8c47e19dbce))

### Changed

- **router:** Extract RouterContext and flatten try_build_router_inner (#235)([a55c06b](https://github.com/madmax983/autumn/commit/a55c06be5f84c72f636fbe7413172f04b78b7571))
- **middleware:** Replace `is_some()` + `unwrap()` with `if let` in `exception_filter.rs` (#71)([17b4676](https://github.com/madmax983/autumn/commit/17b46760757b7fcd7ce650ccae1c2a70dbcc3146))
- **bookmarks:** Replace hand-written API routes with api macro([c66c2e3](https://github.com/madmax983/autumn/commit/c66c2e3f2bef5dd19f51b76d1aef8dcaecf97c4c))

### Documentation

- Add known bug note to Channels panics (#363)([c07d4db](https://github.com/madmax983/autumn/commit/c07d4db5ae16d12f7428860af3c05179abc640a4))
- Clean up bug references in channel docs and tests (#311)([8690e9d](https://github.com/madmax983/autumn/commit/8690e9d7a2447e33b1c7c1df47d32ba94b4d2394))
- Add spec for audit logging (#277)([51da75f](https://github.com/madmax983/autumn/commit/51da75fbf6720fec5571b2e04cbe6a7e1c28a4f3))
- Add DX Audit Report (#251)([25abfdd](https://github.com/madmax983/autumn/commit/25abfdd3659b8c9329b18e25d2b903488b169223))
- Add vantage spec for websocket support (#219)([49edbda](https://github.com/madmax983/autumn/commit/49edbda4ac209a18ba4c5e5c88a6c5b7de03b020))
- Add spec for migration management (#183)([809ac97](https://github.com/madmax983/autumn/commit/809ac97bf1b1a08a81f5bb4a27bc055b63d1ebab))
- Clean up AppState field noise and add module-level docs (#145)([8ff7424](https://github.com/madmax983/autumn/commit/8ff7424807367dcd08d76c80a149288473599220))
- Add vantage spec for custom middleware (S-049) (#156)([f3086dd](https://github.com/madmax983/autumn/commit/f3086dd12f69994ff1d5da0db40202449c1c38c5))
- Add wasm roadmap design (#60)([6c01f76](https://github.com/madmax983/autumn/commit/6c01f76a46069a9044313c432ecd866486d89816))
- Refresh trunk docs and example guides (#41)([48d4b7e](https://github.com/madmax983/autumn/commit/48d4b7e9e66c3b4e53479bd007d5076d723a74e5))
- Add autumn-harvest Phase 1 implementation plan([d091fed](https://github.com/madmax983/autumn/commit/d091fed8fd1b560751abb59333db3db8fa4aed8e))
- Add CRUD API macro implementation plan([1934e44](https://github.com/madmax983/autumn/commit/1934e44aad842e816210dbf9bed76b3418d9b0ff))
- Add CRUD API macro design plan([98c55f8](https://github.com/madmax983/autumn/commit/98c55f885a2f73e99d18f7fd51e18b1ae11e7a80))
- Update CHANGELOG.md for v0.1.0([0ff87b5](https://github.com/madmax983/autumn/commit/0ff87b5fae52bd4b9a710e7c596bbc2227afb31d))

### Styling

- Cargo fmt([f1fe44d](https://github.com/madmax983/autumn/commit/f1fe44d739406f42813b0d954e6a04e25f331aec))

### Testing

- **dag:** Increase DAG builder coverage (#353)([84487ce](https://github.com/madmax983/autumn/commit/84487ce6872078bf517cd92b0232c67468bbeb54))
- Add fallback_404_handler tests for root path and query params (#348)([75c6d76](https://github.com/madmax983/autumn/commit/75c6d7653bdfba13968072d5b069e5f3cd29b642))
- **htmx:** Add edge case tests for HxResponseExt and verify_password (#312)([aacbb30](https://github.com/madmax983/autumn/commit/aacbb305e2bfe589855ba753750b6bede133c8c6))
- Update auth_dos assertion to prove fast response (#303)([46a8fd5](https://github.com/madmax983/autumn/commit/46a8fd5cee00e0eb09c5766142c9179564bfe05b))
- **security:** Add CTF-themed security regression suite (#278)([d07e8bd](https://github.com/madmax983/autumn/commit/d07e8bdf3dbc10fd58d6bb72ff4fc8ce7416a4e6))
- Verify csrf timing fix is verified in existing test (#262)([cbc9bf1](https://github.com/madmax983/autumn/commit/cbc9bf1dfd8076964b35e713f068a1d3fb72137d))
- **security:** Add test for referrer_policy configuration (#213)([f5e8cf7](https://github.com/madmax983/autumn/commit/f5e8cf7548d1b631519796591f984187a7cc366d))
- Add unit tests for Patch<T> enum state matchers (#210)([ee12301](https://github.com/madmax983/autumn/commit/ee123011933d905aa4f340e8adf798d547166395))
- **middleware:** Test state file reading in live reload handler (#143)([1ba174e](https://github.com/madmax983/autumn/commit/1ba174e178b776cb29c9ca5e5a70fec9ee35d699))
- Add missing tests for AutumnError methods in autumn-web (#109)([a821a19](https://github.com/madmax983/autumn/commit/a821a196b0a0e7fd203649ec51d376a6dadd2e61))
- Add compile-pass for repository with hooks + api combined([14847aa](https://github.com/madmax983/autumn/commit/14847aa00ed18d88df55409dbe59f33004dd7578))
- Kill 8 mutation testing survivors in config module (#26)([7a14dc3](https://github.com/madmax983/autumn/commit/7a14dc3f170c8a2657bf03fae2296a6f870f1c08))

### Miscellaneous

- Extract autumn-harvest to separate repo([ba4e342](https://github.com/madmax983/autumn/commit/ba4e3421d87eced7ff8629ffa0b572adb4c28341))
- Temporarily remove reddit-clone example pending autumn-harvest publish([e765eac](https://github.com/madmax983/autumn/commit/e765eac199807e7546de185e3ddc7690f169c56d))
- Clippy clean-up (#338)([89d0d1b](https://github.com/madmax983/autumn/commit/89d0d1be421d71e7d0c211fc04d01077993bbdc3))
- Python cleanup([3186068](https://github.com/madmax983/autumn/commit/3186068c8f95cf6a91b8d8939cfdc6722a9fcbdd))
- Cleanup([3379bcd](https://github.com/madmax983/autumn/commit/3379bcde055bfc513ec72a45823e2e44b8f28c36))
- Clean up files([0873ccb](https://github.com/madmax983/autumn/commit/0873ccba410543a40c2c8f83926e5088011e80df))
- **deps:** Update testcontainers requirement from 0.23 to 0.27 (#270)([072f4c9](https://github.com/madmax983/autumn/commit/072f4c9c9dd02ad880f3a4c85123fd1896bd3b9a))
- **deps:** Bump softprops/action-gh-release from 2 to 3 (#269)([67f56a4](https://github.com/madmax983/autumn/commit/67f56a43b3572221639ff428b635c6c3519307ca))
- **deps:** Update crossterm requirement from 0.28 to 0.29 (#79)([529c195](https://github.com/madmax983/autumn/commit/529c1950f55b4c92bf7cebfba31b28669c1a197d))
- **deps:** Update bcrypt requirement from 0.17 to 0.19 (#75)([edb7248](https://github.com/madmax983/autumn/commit/edb72480fa322d2f7f8618febb055f95748575a2))
- **deps:** Update tokio-cron-scheduler requirement from 0.13 to 0.15 (#78)([a4ee049](https://github.com/madmax983/autumn/commit/a4ee049cc513550183b572b4d74a903835dfbc5c))
- **deps:** Update toml requirement from 0.8 to 1.1 (#14)([80eb617](https://github.com/madmax983/autumn/commit/80eb617cef4ff93e6ae9a7e861b10932cd4afb6f))
- **deps:** Update sha2 requirement from 0.10 to 0.11 (#17)([514578a](https://github.com/madmax983/autumn/commit/514578ac04c8d9c4c461f26b304fbd6ca322b460))
- **deps:** Update reqwest requirement from 0.12 to 0.13 (#15)([80dc749](https://github.com/madmax983/autumn/commit/80dc749048f198ec8a1c0101bdb3254f37161185))
- **deps:** Bump codecov/codecov-action from 5 to 6 (#12)([a5b4bd0](https://github.com/madmax983/autumn/commit/a5b4bd0f9ea7a8712d33b61826add456128ba8f9))
- Clean up test files and encoding issues([63cc397](https://github.com/madmax983/autumn/commit/63cc39743d6eb60f8dc07197a19463f36304eedb))
- Fmt([15ac48d](https://github.com/madmax983/autumn/commit/15ac48d6ddfb5c91c00ec087d192060afe666668))

### Docs

- Fix intra-doc links and add error examples (#88)([0e9dbad](https://github.com/madmax983/autumn/commit/0e9dbadd9fbe988ea2f42a29a25650dfa4fa22a3))

### Echo

- Fix DX audit findings (Macros, 404 Body, Tailwind Warnings) (#294)([7a47630](https://github.com/madmax983/autumn/commit/7a47630986536d36eae87e0cc2a6fed0d233eca6))
- DX Audit for README Setup (#241)([9938abd](https://github.com/madmax983/autumn/commit/9938abdf837aea1b5288d634c0be43d473ccacc1))
- DX Audit Complaint & Fix (#195)([1b80080](https://github.com/madmax983/autumn/commit/1b80080775c63cb88b8a5b91d26e9dd0bfa229a7))
- DX Audit Complaint & Fix (#204)([7144209](https://github.com/madmax983/autumn/commit/7144209dd098b1d2db3e14370f52ceed3df4fa87))

### Wasm

- Fix cookie access, add prelude and wasm tests, and make target-specific dev-deps (#112)([bb49d40](https://github.com/madmax983/autumn/commit/bb49d405d64a498e813f21684fd35e335b368e7d))
## [0.1.0] - 2026-03-26

### Added

- Add Cargo feature flags for optional dependencies (S-044)([f6207c9](https://github.com/madmax983/autumn/commit/f6207c937dd19a7bf3402829a40fdde54b6d257d))
- Add E2E integration test for scaffolded project (S-037)([c09049f](https://github.com/madmax983/autumn/commit/c09049f535a34a4c14e20a0f97c334617e98ff27))
- Add todo-app example with Diesel, Maud, htmx, and Tailwind (S-041)([72e8a89](https://github.com/madmax983/autumn/commit/72e8a8987258672ae54f65e93942bcbedb89261a))
- Implement `autumn setup` — Tailwind CLI download with checksums (S-036)([56af096](https://github.com/madmax983/autumn/commit/56af0968379e370d739c1139e1c41de3726bd4f9))
- Add autumn-cli with project scaffolding and CI (Sprint 9)([2dc8314](https://github.com/madmax983/autumn/commit/2dc8314d3cd892bc6ddf5b00aadde579222cedd6))
- Expand env var overrides to all config fields (S-027)([c7a7782](https://github.com/madmax983/autumn/commit/c7a7782e4f1ef551771407cc6b97b2d8540c16d9))
- Add autumn::prelude module with common re-exports (S-033)([e0e9166](https://github.com/madmax983/autumn/commit/e0e9166670d7a00a1d7e90c6ffa218d571755e86))
- Add SIGTERM handling and shutdown timeout (S-030)([c30fe29](https://github.com/madmax983/autumn/commit/c30fe29a2633cac8ff27a0dc9338771c3d2fdc4c))
- Add health check endpoint with pool status (S-029)([e0c4a87](https://github.com/madmax983/autumn/commit/e0c4a877590c27ece3e8e3d77473f7f1d74650c4))
- Add structured logging via tracing-subscriber (S-028)([a2a40a5](https://github.com/madmax983/autumn/commit/a2a40a5b570624fb95d32064f55064bba163d2ac))
- Add static directory serving via tower-http ServeDir (S-032)([3ccb8a9](https://github.com/madmax983/autumn/commit/3ccb8a9ee10883e99e5c8216eb5c80bfcaea0ee3))
- Embed htmx 2.0.4 and serve at /static/js/htmx.min.js (S-022)([6e51ae9](https://github.com/madmax983/autumn/commit/6e51ae91d2c17a15fd5ffaee7cf463dc4e6c7419))
- Add Tailwind build.rs template and input.css (S-024, S-021)([d5053e2](https://github.com/madmax983/autumn/commit/d5053e25c1e40960cc87fba0f680eb72aa253895))
- Sprint 6 — Db extractor, Maud, Json, Form re-exports (S-017, S-020, S-023, S-031)([0b917ac](https://github.com/madmax983/autumn/commit/0b917acdab24b229a157b66a0f9ac297362d7961))
- Sprint 5 — database pool, #[model] macro, env config overrides (S-016, S-018, S-019)([e28b3fd](https://github.com/madmax983/autumn/commit/e28b3fd22a7d6afe5780ec6594e01846263cec99))
- Sprint 4 — error handling, macro diagnostics, request ID (S-007, S-012, S-011)([04c96bd](https://github.com/madmax983/autumn/commit/04c96bd899c126d7e74087e78928b45ee496b522))
- Sprint 3 — first running Autumn server (#4)([11bb094](https://github.com/madmax983/autumn/commit/11bb09468a190868064e81e8de4a28da6712e5ec))
- Implement routes![] collection macro (S-005)([efc1590](https://github.com/madmax983/autumn/commit/efc15900dd002441fd3517c15e1fdf9e6d5a0d07))
- Add #[post], #[put], #[delete] macros and debug_handler tests (S-003, S-004)([34e80f3](https://github.com/madmax983/autumn/commit/34e80f39e166b5cd1980ffac7934ea69a92ec560))
- Add TOML config file loading with ConfigError (S-026)([41b9573](https://github.com/madmax983/autumn/commit/41b9573cd7d65318402bce3920875136bc740d77))
- Add AutumnConfig struct with serde defaults (S-025) (#2)([4dda5bd](https://github.com/madmax983/autumn/commit/4dda5bd23d6dc132c8623fda5ab8fb64100139bd))
- Implement #[get] route macro with compile-fail tests (S-002) (#1)([66097a9](https://github.com/madmax983/autumn/commit/66097a9808bec4b14b16f08a8fa7a74ad0765052))
- Initialize workspace skeleton with autumn and autumn-macros crates (S-001)([604c348](https://github.com/madmax983/autumn/commit/604c3484286dc1bf4c8096cf9207eb3404c2893d))

### Fixed

- Resolve workspace-root DX issues and polish todo-app UI([d0d45ab](https://github.com/madmax983/autumn/commit/d0d45abf08df288782704bdd24f2f5e113a3dafb))
- Gate maud re-exports behind feature flag in API docs([84d8623](https://github.com/madmax983/autumn/commit/84d862371f4b71a0a009fdad05bc6e1c758b507e))
- Tailwind sha([26bb78f](https://github.com/madmax983/autumn/commit/26bb78f3a918fe53daffa87ac13a4096e3a06384))
- Add reason to #[ignore] attribute (clippy pedantic)([8f70857](https://github.com/madmax983/autumn/commit/8f70857fb64f4b252d53efdd86c2d364a8006101))
- Address code review — .pretty() format, stale doc, test gaps([b229019](https://github.com/madmax983/autumn/commit/b2290196ccbbdbbb64acc81a2dd9a7f895409c16))
- Address code review — explicit Response type, route priority test([209528a](https://github.com/madmax983/autumn/commit/209528a50fa99bd4bc0dc77fc4d9dd02db292795))

### Changed

- Simplify code quality across framework and example app([d28c3b3](https://github.com/madmax983/autumn/commit/d28c3b385cdf3eb6b58a4e3d535d8eccd4a9e130))
- Rename lib identity from autumn to autumn_web([a77a6d0](https://github.com/madmax983/autumn/commit/a77a6d0305fbc1c2b8b62641c3b6f671aa4ae43b))
- Publish as autumn-web on crates.io, keep autumn as lib name([3eb1ae7](https://github.com/madmax983/autumn/commit/3eb1ae7a13574fb7afe976213342d314ec6c4199))

### Documentation

- Add CI, coverage, license, and MSRV badges to README ([bc2eb3a](https://github.com/madmax983/autumn/commit/bc2eb3a4354b386a0ee2ff02745fd83166ff087c))
- Add Sprint 12 story (S-045) and update sprint status([370da00](https://github.com/madmax983/autumn/commit/370da0090e33ff8b2ea96eb2bac6f644f0161f39))
- Add Sprint 11 story definitions and update sprint status([2def24f](https://github.com/madmax983/autumn/commit/2def24fed07da9ac60cf7c5de14c3ce12cd50835))
- Add comprehensive API docs with examples on all public types (S-042)([dc894cd](https://github.com/madmax983/autumn/commit/dc894cd793b55d3fcb13bc7b0cc5cbb12f67541e))
- Add tutorial outline and Chapter 1 — Project Setup (S-040, Sprint 11)([c79b58b](https://github.com/madmax983/autumn/commit/c79b58b68c6ada0717f95549ab36f2b79a4ac6f5))
- Add getting started guide — zero to running app (S-039)([ae41763](https://github.com/madmax983/autumn/commit/ae41763b0afd37e913a0ea00139cdca4f89ea63b))
- Add README with quickstart and maturity warning (S-038)([1ac6798](https://github.com/madmax983/autumn/commit/1ac6798fc824ab173d593c42d429bb33c3daecb8))
- Add story documents for Sprint 10 and update sprint status([8b48585](https://github.com/madmax983/autumn/commit/8b485855ce61187ec742611953eacfc60f6146fc))
- Add story documents for Sprint 8 and update sprint status([f8b72cd](https://github.com/madmax983/autumn/commit/f8b72cd9c4f299d38225fd95273ff840768d7ee5))
- Add story documents for Sprint 7 and update sprint status([9dc1868](https://github.com/madmax983/autumn/commit/9dc1868ab614998f7c3bcfcdab0673cdd8b1f3bf))
- Add story documents for Sprint 6 and update sprint status([ed9d59a](https://github.com/madmax983/autumn/commit/ed9d59a174f26ed31c74989668e2d8f7b9b6abfb))
- Add story documents for Sprint 5 and update sprint status([41a396f](https://github.com/madmax983/autumn/commit/41a396feade34ec3235091db0c784ba056128bdf))
- Add story documents for Sprint 2 (recreated) and Sprint 3([56ac775](https://github.com/madmax983/autumn/commit/56ac775dc1bb0ef85a165f698c15067e0433e949))

### Testing

- Boost coverage from 84% to 91% on framework crate([33f410b](https://github.com/madmax983/autumn/commit/33f410b14ccf4cf21676111388626b392c21b2c5))
- Add missing spec-required tests for htmx serving and static 404([261a4a3](https://github.com/madmax983/autumn/commit/261a4a3b024d00d78fa543f4fd518236b4624f0e))

### Miscellaneous

- Commit CHANGELOG.md back to trunk on release([6b5eb82](https://github.com/madmax983/autumn/commit/6b5eb82b27d3932880f21b3cc3afc0fc29fa8790))
- Add codecov, dependabot, and changelog tooling for v0.1 (#9)([db0d670](https://github.com/madmax983/autumn/commit/db0d6705c6379880fd51c48ae728824530cce5cb))
- Update sprint status — Sprint 2 complete (13/12 pts)([07e0738](https://github.com/madmax983/autumn/commit/07e07387190401f4208f4a3eca1298bcaef5e856))

