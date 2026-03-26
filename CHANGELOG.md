# Changelog

All notable changes to the Autumn framework will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

- Rename lib identity from autumn to autumn_web([a77a6d0](https://github.com/madmax983/autumn/commit/a77a6d0305fbc1c2b8b62641c3b6f671aa4ae43b))
- Publish as autumn-web on crates.io, keep autumn as lib name([3eb1ae7](https://github.com/madmax983/autumn/commit/3eb1ae7a13574fb7afe976213342d314ec6c4199))

### Documentation

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

- Add missing spec-required tests for htmx serving and static 404([261a4a3](https://github.com/madmax983/autumn/commit/261a4a3b024d00d78fa543f4fd518236b4624f0e))

### Miscellaneous

- Update sprint status — Sprint 2 complete (13/12 pts)([07e0738](https://github.com/madmax983/autumn/commit/07e07387190401f4208f4a3eca1298bcaef5e856))

