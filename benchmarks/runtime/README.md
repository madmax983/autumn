# Framework Runtime Benchmark

Reproducible benchmark suite comparing **Autumn** against Spring Boot, Rails,
Django, Phoenix, and Loco on equivalent full-stack application workloads.

## Methodology

Every framework app implements the same five benchmark paths against an
identical Postgres schema and seed dataset:

| Path | Description |
|------|-------------|
| `GET /api/posts` | JSON list — 50 most-recent posts |
| `POST /api/posts` | JSON create with validation |
| `GET /posts` | Server-rendered HTML list |
| `POST /api/posts` (invalid) | Validation-failure path — expects 422 |
| `GET /api/posts/protected` | Auth-protected route — Bearer token check |

Two benchmark **tracks** are run:

### Comparable Infrastructure Track

All apps run with identical constraints:

- Same Postgres 16 instance and schema.
- Same container CPU/memory limits (2 vCPU, 512 MiB RAM).
- Same seed dataset (1 000 posts, 1 API token).
- Same k6 load profile (VUs, duration, ramp shape).
- Production-mode configuration enabled for each framework.

### Idiomatic Framework Track

Each framework may use its recommended production setup. All deviations from
the comparable track are documented in the framework's `VERSIONS` file and in
the results report. No framework receives shortcuts unavailable to the others
(e.g., Autumn does not disable middleware that others keep).

## Framework Versions

See each framework's `VERSIONS` file for exact versions:

| Framework | Runtime | File |
|-----------|---------|------|
| **Autumn** | Rust 1.88 + Axum 0.8 | [`autumn/VERSIONS`](autumn/VERSIONS) |
| **Spring Boot** | Java 21 + Tomcat | [`spring-boot/VERSIONS`](spring-boot/VERSIONS) |
| **Rails** | Ruby 3.3 + Puma 6 | [`rails/VERSIONS`](rails/VERSIONS) |
| **Django** | Python 3.12 + Gunicorn/Uvicorn | [`django/VERSIONS`](django/VERSIONS) |
| **Phoenix** | Elixir 1.17 + Bandit | [`phoenix/VERSIONS`](phoenix/VERSIONS) |
| **Loco** | Rust 1.88 + Axum 0.7 | [`loco/VERSIONS`](loco/VERSIONS) |

OS, CPU, memory, and container limits are recorded in the results report
generated alongside each benchmark run.

## Infrastructure

```
benchmarks/runtime/
├── docker-compose.yml       # All six apps + Postgres (comparable track)
├── schema/init.sql          # Canonical Postgres schema
├── seed/seed.sql            # Deterministic seed data (1 000 posts)
├── autumn/                  # Autumn (Rust) app
├── spring-boot/             # Spring Boot (Java) app
├── rails/                   # Rails (Ruby) app
├── django/                  # Django (Python) app
├── phoenix/                 # Phoenix (Elixir) app
├── loco/                    # Loco (Rust) app
└── load/
    ├── k6/
    │   ├── json-crud.js      # CRUD JSON API test
    │   ├── html-page.js      # Server-rendered HTML test
    │   ├── validation-fail.js # Validation error path test
    │   └── auth-protected.js # Authenticated route test
    ├── run.sh                # Local k6 orchestration script
    └── run-docker.ps1        # Dockerized k6 runner for PowerShell/Windows
```

## Metrics

Each k6 run captures:

- **p50 / p95 / p99 latency** — from k6's built-in percentile tracking.
- **Sustained throughput** — requests/s before the error rate rises above 1 %.
- **Error rate under load** — `http_req_failed` rate.

Supplementary metrics (collected manually outside k6):

| Metric | How |
|--------|-----|
| Cold start time | `docker compose up <app>`; time until first 200 on `/health` |
| Warm restart time | Send SIGTERM; time until healthy again |
| Idle RSS | `docker stats --no-stream` after 30 s idle |
| Memory under load | `docker stats` during a 60 s k6 run |
| Container image size | `docker image ls` after build |
| Build time | `time docker compose build <app>` |

## Running the Benchmark

See [Tracks](#tracks) below for the full step-by-step workflow. Quick summary:

```powershell
# Comparable track - all frameworks at once
cd benchmarks/runtime
docker --context default compose up -d --build autumn spring-boot rails django phoenix loco
docker --context default compose ps
.\load\run-docker.ps1 all -Vus 50 -Duration 60s

# Repeat the full matrix 5 times for a more stable sample.
.\load\run-docker.ps1 all -Vus 50 -Duration 60s -Repeat 5
```

Results land in `results/docker-k6_<timestamp>/`. `aggregate.csv` keeps one row
per run/framework/scenario, and `aggregate-summary.csv` reports medians across
the repeated runs. Fill in [`RESULTS.md`](RESULTS.md) after the run.

## Tracks

### Running the Comparable Infrastructure Track

```powershell
cd benchmarks/runtime

# 1. Build and start Postgres plus all framework apps.
docker --context default compose up -d --build autumn spring-boot rails django phoenix loco

# 2. Wait for health checks (all apps expose /health).
docker --context default compose ps

# 3. Run the full suite with Dockerized k6.
.\load\run-docker.ps1 all -Vus 50 -Duration 60s
```

Results are written to `results/docker-k6_<timestamp>/<framework>/` and summarized
in `results/docker-k6_<timestamp>/aggregate.csv`. When `-Repeat` is greater than
1, per-run outputs are written under `run-01`, `run-02`, etc., and
`aggregate-summary.csv` contains the median throughput, p95, average latency,
check pass rate, and HTTP failure rate for each framework/scenario.

The Compose Postgres service raises `max_connections` for this all-services
track. Otherwise idle pools from the six apps can exhaust the default Postgres
connection budget and poison later runs.

### Running Against a Single Framework

```bash
cd benchmarks/runtime/load
./run.sh autumn http://localhost:8001 --vus 50 --duration 60s
./run.sh rails  http://localhost:8003 --vus 20 --duration 30s
```

```powershell
cd benchmarks/runtime
.\load\run-docker.ps1 autumn -Vus 50 -Duration 60s
.\load\run-docker.ps1 rails -Vus 20 -Duration 30s
```

### Running the Idiomatic Framework Track

Each framework has a documented "idiomatic" command in its `VERSIONS` file.
Deviations from the comparable track are listed per framework. Run them
individually and record the extra configuration in the results report.

## Building Individual Apps

Each framework app can be built from a clean checkout:

### Autumn (Rust)
```bash
cd benchmarks/runtime/autumn
# Requires: Rust 1.88+, libpq (or pq-sys bundled)
DATABASE_URL=postgres://benchmark:benchmark@localhost:5432/benchmark \
  cargo run --release --bin bench-autumn
```

### Spring Boot (Java)
```bash
cd benchmarks/runtime/spring-boot
# Requires: JDK 21+, Maven 3.9+
DATABASE_URL=postgres://benchmark:benchmark@localhost:5432/benchmark \
  mvn spring-boot:run -Dspring-boot.run.jvmArguments="-Xmx256m"
```

### Rails (Ruby)
```bash
cd benchmarks/runtime/rails
# Requires: Ruby 3.3+, Bundler
bundle install
DATABASE_URL=postgres://benchmark:benchmark@localhost:5432/benchmark \
RAILS_ENV=production SECRET_KEY_BASE=changeme RAILS_MAX_THREADS=20 \
  bundle exec rails db:migrate && bundle exec puma
```

### Django (Python)
```bash
cd benchmarks/runtime/django
# Requires: Python 3.12+
pip install -r requirements.txt
DATABASE_URL=postgres://benchmark:benchmark@localhost:5432/benchmark \
DJANGO_SETTINGS_MODULE=benchapp.settings \
  python manage.py migrate && gunicorn benchapp.wsgi:application \
    --worker-class uvicorn.workers.UvicornWorker --workers 4 --bind 0.0.0.0:8080
```

### Phoenix (Elixir)
```bash
cd benchmarks/runtime/phoenix
# Requires: Elixir 1.17+, Erlang/OTP 27
mix deps.get
DATABASE_URL=ecto://benchmark:benchmark@localhost/benchmark \
SECRET_KEY_BASE=$(mix phx.gen.secret) \
  mix phx.server
```

### Loco (Rust)
```bash
cd benchmarks/runtime/loco
# Requires: Rust 1.88+
DATABASE_URL=postgres://benchmark:benchmark@localhost:5432/benchmark \
  cargo run --release -- start
```

## Seeding

All apps use the same seed dataset via `benchmarks/runtime/seed/seed.sql`.
With Docker Compose the seed is applied automatically at first start
via `docker-entrypoint-initdb.d/`.

To reseed manually:
```bash
psql postgres://benchmark:benchmark@localhost:5432/benchmark \
  -f benchmarks/runtime/seed/seed.sql
```

Or via the Autumn seed binary:
```bash
cd benchmarks/runtime/autumn
DATABASE_URL=postgres://benchmark:benchmark@localhost:5432/benchmark \
  cargo run --bin seed
```

## CI / Build Verification

The `scripts/check-benchmarks.sh` script verifies structural completeness:

```bash
./scripts/check-benchmarks.sh
```

It checks that every framework directory, Dockerfile, migrations path, load
script, and README section is present. This gate runs in CI to ensure the
benchmark apps do not silently rot.

## Caveats and Framework-Specific Notes

- **JVM warm-up**: Spring Boot's p50/p95 during the first 10 s of a run are
  meaningfully higher than steady state due to JIT compilation. Latency charts
  should exclude the first 10 s or use a ramp-up stage.
- **GIL / thread model**: Django uses Gunicorn + Uvicorn workers (ASGI).
  Thread counts are held equal to Rust worker counts (4) in the comparable
  track.
- **BEAM scheduling**: Phoenix/Elixir's BEAM scheduler maps well to container
  CPU limits. Pool sizes are set to `20` for all frameworks in the comparable
  track.
- **Autumn vs. Loco**: Both are Rust + Axum stacks. The difference measures
  framework overhead and feature assumptions, not the language.
- **No hello-world shortcuts**: Every app performs at least one Postgres query
  per request. Static-file serving is disabled or irrelevant for all JSON
  endpoints.

## Results

Benchmark results are stored in `results/` and ignored by Git. Each run
produces a timestamped directory:

```
results/docker-k6_20260512_103000/
├── aggregate.csv
├── aggregate-summary.csv
├── autumn/
│   ├── json-crud-summary.json
│   ├── html-page-summary.json
│   ├── validation-fail-summary.json
│   └── auth-protected-summary.json
├── spring-boot/...
└── ...
```

A rendered comparison report template lives at
[`RESULTS.md`](RESULTS.md) — fill it in after each run.
