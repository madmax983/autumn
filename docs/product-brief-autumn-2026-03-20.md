# Product Brief: autumn

**Date:** 2026-03-20
**Author:** markm
**Version:** 1.0
**Project Type:** library
**Project Level:** 4

---

## Executive Summary

Autumn is an opinionated, convention-over-configuration web application framework for Rust that assembles proven crates (Axum, Maud, Tailwind, htmx, Diesel, Postgres) into a Spring Boot-style developer experience — proc-macro magic by default, hard escape hatches when you need them. It's for Rust developers who want to build complete, production web applications without wiring together a dozen crates and making hundreds of infrastructure decisions before writing their first endpoint. It matters because Rust has no framework that owns this space: every existing option is either too thin (just a router), too narrow (API-only), or too experimental to bet a business on — and Spring Boot proved twenty years ago that opinionated defaults with clean escape hatches is how you win ecosystems.

---

## Problem Statement

### The Problem

Rust has world-class crates for every layer of a web application — but no framework that assembles them into a coherent, opinionated whole. The result is that building a full web app in Rust today is an exercise in yak-shaving: you spend your first few hours (or days) making infrastructure decisions instead of writing your application.

**What a Rust developer actually goes through today:**

You want to build a simple CRUD web app — users, a database, some forms, maybe an API. In Rails, Django, or Spring Boot, you'd have a working app with a database in under ten minutes. In Rust, here's what happens:

1. You pick an HTTP framework. Axum? Actix-web? Poem? You spend an hour reading comparisons. You pick Axum because it seems to have momentum.
2. You need a database. Diesel or SQLx? Diesel has the stronger query builder but its async story is complicated. SQLx is async-native but less type-safe at the query level. You read three blog posts and pick one.
3. You need a connection pool. Deadpool? bb8? r2d2? Each works slightly differently with your ORM choice. You wire it up manually, writing the extractor boilerplate to thread a connection into your handlers.
4. You need configuration. There's config, figment, dotenvy, or just roll your own with toml. You write a config struct, derive Deserialize, write the loading code, figure out how to layer env vars on top of file config.
5. You need error handling. Your handlers return Result, but Axum needs impl IntoResponse. You write an error enum, derive thiserror, implement IntoResponse, write From impls for every error type you encounter. You hit a turbofish problem. You spend 45 minutes on it.
6. You need logging. tracing is the obvious choice but you still have to set up the subscriber, decide on format (JSON? pretty-print?), wire in request-level spans, add trace IDs.
7. You want some HTML. You evaluate Tera, Askama, Maud, and Minijinja. You pick one. You figure out how to serve static files alongside it. You realize you need htmx for interactivity. You manually add the script tag. You want Tailwind, but that requires a Node.js toolchain or the standalone CLI, and now you're managing two build systems.
8. You have no health check, no graceful shutdown, no CORS configuration. Each one is another fifteen minutes of boilerplate.

By the time you've written your first actual endpoint, you've made 30+ decisions, read a dozen READMEs, written hundreds of lines of glue code, and your main.rs is 150 lines of setup before a single route. A developer doing the same thing in Spring Boot wrote `@GetMapping("/users")` and went home.

The deeper problem isn't any individual crate — they're all excellent. The problem is that nobody has taken responsibility for the decisions between them. Every Rust web app is a bespoke integration project. Autumn is the framework that makes those decisions for you, and gets out of the way when you disagree.

### Why Now?

**The crate winners have emerged.** In 2023, the Rust HTTP framework landscape was still genuinely contested — Actix-web, Axum, Warp, Poem, Rocket were all plausible choices. Today, Axum has effectively won. It's backed by the Tokio team, it has the most momentum, and the ecosystem has built around it (Tower middleware, broad extractor support). Diesel has similarly solidified its position with diesel-async maturing enough to use in production. Two years ago, picking a stack meant gambling. Today, the right answers are clear enough to be opinionated about.

**The frontend story resolved without JavaScript.** htmx went from niche to mainstream, proving that server-rendered HTML with declarative interactivity is a legitimate architecture — not a throwback. Tailwind shipped a standalone CLI that doesn't require Node.js. Maud has been stable for years. The pieces for a complete, zero-JavaScript-toolchain web application stack all exist now and are all production-tested. This combination wasn't coherent or mature enough to bet on in 2023.

**Rust is winning in adjacent spaces but losing in web.** Rust adoption has exploded in cloud infrastructure, CLI tooling, data systems, and embedded — but web application development remains dominated by Go, TypeScript, Python, and Java. The reason isn't that Rust is too hard for web. It's that the web DX is too hard for Rust. Teams evaluating Rust for a new web project look at the integration burden described above, compare it to `rails new` or `spring init`, and choose something else. The language is ready. The ecosystem is ready. The framework is missing.

**AI-assisted development changes the build calculus.** The proc-macro-heavy, convention-over-configuration framework that Autumn aspires to be is genuinely hard to build. Two years ago, it would have been a multi-person, multi-year effort. Today, with AI-assisted development workflows, a small team (or a single motivated developer) can realistically build and maintain a framework of this scope. The tooling has caught up to the ambition.

### Impact if Unsolved

**Rust stays a "systems language" indefinitely.** Not because it can't do web — it obviously can — but because the first-hour experience drives adoption, and right now the first hour of Rust web development is spent fighting integration complexity instead of building features. Languages win ecosystems with frameworks, not with libraries. Java didn't dominate enterprise because of the JVM — it dominated because of Spring. Ruby didn't dominate startups because of its syntax — it dominated because of Rails. Rust's web story today is "here are 50 excellent crates, good luck." That's a library ecosystem, not a framework ecosystem, and library ecosystems don't win mainstream adoption.

**Teams that want Rust's guarantees for web applications choose Go instead.** Go's web story isn't exciting — net/http plus a router plus database/sql — but it's simple, well-documented, and requires very few decisions. For a team lead evaluating Rust vs. Go for a new web service, "Rust is faster and safer but you'll spend your first week on plumbing" is a losing pitch. Autumn changes that pitch to "Rust is faster, safer, and you ship your first endpoint in five minutes."

**The ecosystem keeps duplicating effort.** Without a framework to converge on, every Rust web project reinvents the same glue code. Connection pool wiring, error-to-response mapping, config loading, health checks, graceful shutdown — thousands of developers are writing the same hundred lines of boilerplate independently, each with slightly different bugs. That's wasted human effort at ecosystem scale.

---

## Target Audience

### Primary Users

**The Spring Boot / Rails / Django Developer Adopting Rust**

This is the person Autumn is built for. They're an experienced web developer — probably 3-10 years in — who has shipped production applications in a framework that made decisions for them. They know what a good DX feels like. They've chosen Rust (or are evaluating it) for the performance, safety, or type system, but they don't want to give up the productivity they had in their previous stack.

They're coming from Spring Boot (Java/Kotlin), Rails (Ruby), Django (Python), Laravel (PHP), or ASP.NET (C#). They know what convention-over-configuration means because they've lived inside it. When they see Autumn's `#[get("/users")]` and `autumn.toml`, they immediately understand the contract. They don't need to be convinced that opinionated frameworks are good — they need to be convinced that one exists in Rust.

**What they're building:** startup MVPs, internal tools, SaaS products, CRUD applications — the bread-and-butter web apps that constitute 90% of what gets built with Spring Boot. Not research projects, not static sites, not microservices-for-the-sake-of-microservices. Real business applications that need a database, some forms, an API, and to go to production.

**Their current pain:** they tried Rust for web once, spent a day on plumbing, and went back to their old stack. Or they haven't tried yet because every "getting started with Rust web" tutorial is 200 lines before the first route. Autumn's job is to make them stay.

### Secondary Users

**The Rust Systems Developer Who Needs A Web App**

This person knows Rust well — maybe they've built CLIs, libraries, or infrastructure tools — but they've never built a web application in it. They're not coming from Spring Boot; they're coming from the Rust ecosystem itself. They've heard of Axum but never wired up a connection pool. They know Serde but have never thought about IntoResponse.

They don't need to be taught Rust. They need to be taught web, and they'd rather the framework handle the parts they don't care about (asset serving, CORS, health checks) so they can focus on the parts they do (their business logic, their data model). Autumn's proc macros and smart defaults serve them well, but they'll hit the escape hatches faster because they're comfortable writing Rust directly.

**What they're building:** internal dashboards, admin panels for their existing Rust services, side projects that need a UI, monitoring tools. Often a web frontend for something that already exists as a CLI or library.

### User Needs

1. **Time to first endpoint under 5 minutes** — the framework must eliminate the infrastructure decision overhead that currently dominates the first-hour experience
2. **Convention-over-configuration with escape hatches** — sensible defaults for everything, but the ability to override any decision when the default doesn't fit
3. **Production-ready out of the box** — logging, health checks, error handling, graceful shutdown should be on by default, not afterthoughts

### Not The Target (Yet)

- **Junior Rust developers.** Autumn hides complexity behind proc macros. Someone who doesn't yet understand async runtimes, connection pools, or HTTP routing will hit confusing errors when they stray from the happy path. Autumn is something you graduate into, not something you learn through.
- **Microservices-only teams.** If you're building tiny JSON services behind a gateway and never render HTML, Autumn's application-first opinions (Maud, Tailwind, htmx) are overhead you don't want.
- **Teams needing a React/Vue/Svelte SPA backend.** Autumn's opinion is server-rendered HTML. If your architecture is a Rust API backend with a JavaScript SPA frontend, you'd be fighting the framework's grain.

---

## Solution Overview

### Proposed Solution

Autumn is an opinionated web application framework for Rust that assembles proven crates into a cohesive, convention-over-configuration developer experience with five levels of escape:

1. **Override config** via TOML or env vars
2. **Add custom middleware** via Tower
3. **Mount raw Axum routes** alongside Autumn routes
4. **Replace subsystems** by implementing traits
5. **Don't use Autumn** — cherry-pick individual crates

### Key Features

**v0.1 Core (Must-Have):**

- **Zero-boilerplate project startup** — `autumn new my-app` produces a compiling, running, styled web application with a database connection
- **Annotation-driven routing** — `#[get("/users")]` on a function makes it a route with zero registration boilerplate
- **Automatic dependency wiring** — handler function signatures declare what they need (`Db`, `Path<T>`, `Form<T>`, `Json<T>`) and the framework wires it
- **Convention-based configuration** — `autumn.toml` with sensible defaults, env var overrides
- **Integrated rendering stack** — Maud + Tailwind CSS + htmx out of the box, styled and working
- **JSON escape hatch** — return `Markup` for HTML, `Json<T>` for JSON; the return type is the contract
- **Transparent error handling** — `?` works everywhere with no turbofish, errors become appropriate HTTP responses automatically
- **Production defaults** — structured logging, health checks, graceful shutdown, request tracing on by default
- **Escape hatches at every level** — override any decision without ejecting from the framework

**v1.0 Additions (Stability Commitment):**

- CORS configuration, custom middleware builder, raw Axum router merging
- Trait-based subsystem replacement, dev/prod profiles
- Migration management, error page overrides, CSRF protection
- API stability guarantee (semver commitment)
- Comprehensive documentation (API reference, architecture guide, escape hatch cookbook)

### Value Proposition

It's not about what you can build — it's about what you don't have to decide. The raw Axum approach requires ~30 integration decisions before writing your first endpoint. Autumn makes those decisions once, correctly, and lets you override any of them. The value is the integrated stack where the error handling knows about the rendering layer, where the config system knows about the connection pool, where the build system knows about the CSS framework. That integration is the product.

**Specific advantages over DIY:**
- **Time to first endpoint:** 5 minutes vs. 2-4 hours
- **Consistency across projects:** one way to do config, errors, routing
- **Upgrade path:** Autumn handles upstream migrations
- **No lock-in:** escape hatches mean you never paint yourself into a corner

---

## Business Objectives

### Goals

- **6 months:** Ship v0.1 to crates.io — compiles, runs, delivers on the core promise. A stranger can `autumn new my-app` and have a working application. Not mass adoption — just a real, published, documented framework.
- **1 year:** Credibility — Autumn is a recognized name in the Rust web ecosystem. Present in conversations alongside Loco, Poem, and raw Axum. Issues and PRs from people who aren't the author.
- **2 years:** Standard — Autumn is a default recommendation for Rust web applications. v1.0 with stability guarantees, multiple contributors with merge access, a "built with Autumn" showcase of real applications.

### Success Metrics

- **6 months:** v0.1 on crates.io, compiles on stable Rust, documented, at least one non-trivial example application
- **1 year:** 200+ GitHub stars, 1,000+ crates.io downloads, 10+ issues from external users, at least one conference talk submitted
- **2 years:** 2,000+ GitHub stars, 20,000+ crates.io downloads, 5+ active contributors, production use by teams not known personally

### Business Value

Autumn is an open-source ecosystem play. The goal is to become the default opinionated web application framework for Rust. This is a reputation and ecosystem bet, not a direct commercial product. The business value is indirect but compounding: it establishes credibility in the Rust web ecosystem, attracts contributors and community, and creates a foundation that downstream products can build on.

**Not optimizing for:** revenue, hype, or feature count. A focused framework that does 10 things well beats a sprawling one that does 50 things poorly.

---

## Scope

### In Scope

**v0.1 (6-Month Ship):**

- `autumn` crate with re-exports (`cargo add autumn` compiles)
- `autumn-macros` proc macro crate (`#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[autumn::main]`, `#[derive(Model)]`)
- `autumn-cli` with `autumn new` project scaffolding
- Route discovery via `inventory` or `linkme` (zero-registration)
- `Db` extractor with diesel-async connection pool
- `Path<T>`, `Form<T>`, `Json<T>` extractors with Autumn error handling
- Config system (`autumn.toml` + env var overrides + framework defaults)
- Maud integration (`Markup` return type → HTML response)
- htmx bundled and served automatically
- Tailwind CSS standalone CLI integration via `build.rs`
- Static asset serving (`static/` → `/static/`)
- Error handling (`AutumnError` + blanket `From` + optional `IntoAutumnError`)
- Health check (`GET /health`), structured logging, graceful shutdown
- Documentation (README, getting started guide, tutorial)
- Non-trivial example application

**v1.0 (Stability Commitment):**

- CORS (configurable, permissive in dev, locked in prod)
- Custom middleware builder, raw Axum route merging
- Trait-based subsystem replacement (documented, stable extension traits)
- Dev/prod profiles in `autumn.toml`
- Migration management (auto-run in dev)
- Default error pages with override mechanism
- Tailwind config override support
- CSRF protection, secure headers
- Semver stability guarantee
- Comprehensive docs (API reference, architecture guide, escape hatch cookbook, Axum migration guide)

### Out of Scope

- **ORM abstraction.** Autumn uses Diesel, not an abstraction over ORMs. No SQLx support in v1.
- **Frontend framework.** No WASM compilation, client-side routing, or virtual DOM. Not competing with Leptos/Dioxus/Yew.
- **Multiple databases.** Postgres only. No MySQL, SQLite, or MongoDB.
- **GraphQL.** API escape hatch is REST/JSON only. Use async-graphql via the raw Axum escape hatch if needed.
- **Deployment tooling.** No Docker generation, Kubernetes manifests, or cloud provider integrations. Single binary, deploy however you want.
- **Dependency injection.** Rust's type system (traits, generics, cargo features) already solves what DI solves in Java.
- **Runtime plugin loading.** No dynamic libraries, no hot-reload. Extensions happen at compile time.

### Future Considerations

- Content negotiation (`Negotiated<T>` return type based on Accept header)
- WebSocket support (`#[ws("/path")]` macro)
- Background jobs / scheduled tasks (`#[schedule("0 * * * *")]`)
- Starter / feature system (`autumn = { features = ["redis"] }`)
- Auto-generated admin panel (Django Admin equivalent)
- OpenTelemetry integration
- i18n / localization
- File uploads with streaming support
- Session management (Postgres or Redis-backed)
- Rate limiting middleware

---

## Key Stakeholders

**Tier 1: High Influence, Must Actively Manage**

- **Mark (Creator/Sole Maintainer)** — High influence. Single point of failure. The most critical stakeholder and the most likely risk vector (attention management, ADHD hyperfocus patterns). Mitigation: tight v0.1 scope, modest 6-month goals, parallel workstreams for when hard stuff is stuck.
- **Upstream crate maintainers (Axum/Tokio team, Diesel team, Maud maintainer)** — High influence. Their decisions directly affect Autumn. Relationship is currently one-directional. Goal: become a responsible downstream consumer, contribute fixes back, introduce the project at the 1-year credibility milestone.

**Tier 2: Medium Influence, Keep Informed**

- **Rust web community (r/rust, Rust Users Forum, Zulip)** — Medium influence. Reputation made or broken here. Don't announce until v0.1 is on crates.io with an excellent README. Engage genuinely with feedback; the community rewards humility and punishes defensiveness.
- **Adjacent framework authors (Loco, Poem, Rocket, Actix-web)** — Medium influence. Not adversaries — collaborators in proving Rust is viable for web. Study their issue trackers and documentation gaps. Position alongside, not against.
- **pgray / MADstack** — Medium influence. Already validated the crate combination. Natural early ally. Reach out once v0.1 exists.

**Tier 3: Lower Influence, Monitor**

- **Rust project / lang team** — Low influence. Monitor RFCs affecting proc macros and build system. Stay on stable Rust.
- **Potential corporate adopters** — Low influence (until v1.0). Build for individuals first; corporate adoption follows ecosystem credibility.
- **htmx / Tailwind communities** — Low influence. Pin to specific versions. Test against pre-releases.

**The Stakeholder Nobody Talks About: Future You.** Every design decision should be evaluated not just by "is this the right architecture?" but by "will I be able to maintain this when I'm less excited about it?" This is why the scope is tight, the proc macros are limited to three core patterns, and the Out of Scope list exists.

---

## Constraints and Assumptions

### Constraints

- **Team size: one.** Single developer, no co-maintainer, no funding. Every design must pass: "can one person build, document, and maintain this?"
- **Time: side project, not full-time.** 10-20 hours/week of focused development. The 6-month v0.1 timeline assumes this cadence.
- **Stable Rust only.** No nightly features. Non-negotiable for production credibility.
- **Postgres only.** Single database target. No abstraction over multiple backends.
- **No external runtime dependencies.** Single binary at runtime. Tailwind CLI is build-time only. htmx is embedded.
- **Must not fork upstream crates.** If upstream doesn't do what Autumn needs: contribute, work around, or accept the limitation.
- **Budget: zero.** No hosting costs beyond crates.io and GitHub free tier.

### Assumptions

- **Axum remains the Rust HTTP framework winner** (Confidence: High). Backed by Tokio team, strongest momentum. Monitor: release cadence, community sentiment.
- **diesel-async is production-ready** (Confidence: Medium-High). Works today, actively maintained, but younger than sync Diesel. Fallback: `spawn_blocking` with sync Diesel.
- **inventory or linkme works reliably on stable across platforms** (Confidence: Medium). Linker tricks are platform-dependent. Fallback: explicit registration with macro assistance.
- **Server-rendered HTML + htmx is a durable architecture** (Confidence: Medium-High). htmx has gone mainstream, but web ecosystems are faddish. Escape hatch always exists.
- **Tailwind standalone CLI remains available and stable** (Confidence: High). Strong commercial backing. Fallback: npm-based CLI or Rust-native compiler.
- **Rust developers want an opinionated web framework** (Confidence: Medium). Deepest assumption, hardest to validate. Rust culture values explicitness and control. First thing to learn from community response at launch.
- **One developer can build a credible framework** (Confidence: Medium). Monitor: honest self-assessment at 3-month mark. If proc macros aren't working by month 3, scope needs to shrink.

---

## Success Criteria

**Qualitative signals that matter more than metrics:**

- **Someone builds something real with Autumn and doesn't tell you about it.** Silent adoption — discovered by accident via GitHub search or blog post. The framework was good enough to be unremarkable.
- **The question changes.** "What should I use for a web app in Rust?" includes Autumn as a legitimate option. Not dominant — just present and credible.
- **A stranger opens a good issue.** Not a bug report — a design issue. They've used the framework long enough to have opinions about how it should evolve.
- **You reach for Autumn yourself.** Not to test it — but because you're starting a new project and your instinct is `autumn new` instead of `cargo new` plus an hour of wiring.
- **Someone says "I tried Rust for web once and gave up, but Autumn made it click."** Converting developers from "Rust is great but not for web" to "Rust is great and Autumn makes web easy."
- **Someone forks Autumn and makes different choices.** The architecture was clean enough to build on, the opinions clear enough to disagree with.

**What success does NOT feel like:** a viral launch post with 500 stars and no one using it six months later. Being the framework people recommend but nobody chooses. A technically impressive project that only other framework authors appreciate.

**Autumn succeeds when it becomes boring.** The obvious, unremarkable, correct choice — the thing you use because it works, not because it's exciting.

---

## Timeline and Milestones

### Target Launch

v0.1 on crates.io within 6 months (by September 2026). v1.0 aspirational at 2 years (March 2028).

### Key Milestones

**Month 1-2: FOUNDATION (blocks everything)**
- Workspace setup & crate structure (Week 1)
- `#[get]`/`#[post]` proc macros → route handler generation (Weeks 2-4) — **HIGHEST RISK**
- Route discovery via inventory/linkme (Weeks 4-6)
- `#[autumn::main]` → boots Axum with discovered routes

**Month 2-3: DATABASE LAYER (blocks realistic examples)**
- diesel-async connection pool integration
- `Db` extractor
- Error handling machinery (`AutumnError` + blanket `From`) — **HIGH RISK**
- `#[derive(Model)]` proc macro

**3-MONTH GUT CHECK:** Do proc macros work? Does `Db` extractor work? Would you use this yourself?

**Month 3-4: RENDERING & ASSETS (blocks "looks like a real app")**
- Maud integration (`Markup` → HTML response)
- Tailwind `build.rs` pipeline (standalone CLI auto-download)
- htmx bundling & static asset serving
- `Form<T>` extractor

**Month 4-5: PRODUCTION DEFAULTS (blocks "feels like a framework")**
- Config system (`autumn.toml` + env vars + defaults)
- Structured logging, health check, graceful shutdown
- `Json<T>` return type (JSON escape hatch)

**Month 5-6: SHIP (polish)**
- `autumn-cli` (`autumn new` scaffolding) — cuttable; fallback to cargo-generate template
- Example application, documentation, CI pipeline
- Publish to crates.io

**Parallelizable work (for when proc macros are stuck):**
- Config loading library
- Tailwind `build.rs` pipeline
- Static asset serving
- Health check endpoint
- `autumn-cli` scaffolding

---

## Risks and Mitigation

- **Risk:** Creator loses interest / focus before v0.1 ships
  - **Likelihood:** Medium-High (honest pattern recognition: ADHD hyperfocus is an asset for sprints and a risk for the long middle)
  - **Mitigation:** Tight v0.1 scope, modest 6-month goal ("it compiles and runs" not "people are using it"), parallel workstreams for productive context-switching, building the example app alongside the framework for visible reward. Kill signal: if you dread opening the repo for two consecutive weeks, the project is dying.

- **Risk:** Rust community rejects the premise (opinionated frameworks aren't wanted)
  - **Likelihood:** Medium (lowest-confidence assumption from Constraints section)
  - **Mitigation:** Target users adopting Rust, not just the existing community. Launch quietly with working code, not a manifesto. Listen to feedback genuinely. Kill signal: if early response is consistently "I'd rather wire Axum myself" with no curiosity about the approach.

- **Risk:** inventory/linkme breaks on a major platform or Rust version
  - **Likelihood:** Medium (linker-level magic is inherently fragile)
  - **Mitigation:** Design route registration as a trait with two implementations: auto-discovery and explicit registration. If discovery breaks, explicit registration is a one-line change per app. Magic degrades gracefully.

- **Risk:** Axum ships breaking changes that invalidate Autumn's generated code
  - **Likelihood:** Medium (Axum is pre-1.0 and does make breaking changes)
  - **Mitigation:** Insulate generated code behind Autumn's own traits. Macros generate code that calls Autumn types, not Axum types directly. An Axum migration becomes internal refactoring, not user-facing breakage.

- **Risk:** Someone else ships a credible Spring Boot-for-Rust first
  - **Likelihood:** Low-Medium (Loco is closest but makes different architectural choices)
  - **Mitigation:** Study what they ship. If it's good and serves the same audience, contribute to it instead. The goal is "Rust gets a Spring Boot" — it doesn't have to be yours.

---

## Next Steps

1. Create Product Requirements Document (PRD) - `/bmad:prd`
2. Conduct competitive research on Loco, Poem, Rocket (optional) - `/bmad:research`
3. Create system architecture - `/bmad:architecture`

---

**This document was created using BMAD Method v6 - Phase 1 (Analysis)**

*To continue: Run `/bmad:workflow-status` to see your progress and next recommended workflow.*
