# Benchmark Results

> Fill in this template after each benchmark run.
> Raw k6 JSON output files are in `load/results/<timestamp>/`.

## Run Metadata

| Field | Value |
|-------|-------|
| Date | _YYYY-MM-DD_ |
| Host OS | _e.g. Ubuntu 24.04 LTS_ |
| CPU | _e.g. AMD EPYC 7742 4 vCPU_ |
| RAM | _e.g. 16 GiB_ |
| Docker Engine | _e.g. 27.x_ |
| Container limits | 2 vCPU, 512 MiB per app |
| Postgres | 16-alpine |
| k6 version | _e.g. 0.55.0_ |
| Track | comparable-infrastructure / idiomatic |
| VUs | _e.g. 50_ |
| Duration | _e.g. 60s_ |

## Cold Start Time

| Framework | Time to first 200 /health |
|-----------|--------------------------|
| Autumn | |
| Spring Boot | |
| Rails | |
| Django | |
| Phoenix | |
| Loco | |

## Idle RSS (after 30 s idle)

| Framework | RSS |
|-----------|-----|
| Autumn | |
| Spring Boot | |
| Rails | |
| Django | |
| Phoenix | |
| Loco | |

## Container Image Size

| Framework | Image size |
|-----------|-----------|
| Autumn | |
| Spring Boot | |
| Rails | |
| Django | |
| Phoenix | |
| Loco | |

## JSON CRUD — Latency (p50 / p95 / p99) and Throughput

| Framework | p50 | p95 | p99 | req/s | Error rate |
|-----------|-----|-----|-----|-------|------------|
| Autumn | | | | | |
| Spring Boot | | | | | |
| Rails | | | | | |
| Django | | | | | |
| Phoenix | | | | | |
| Loco | | | | | |

## HTML Page — Latency (p50 / p95 / p99) and Throughput

| Framework | p50 | p95 | p99 | req/s | Error rate |
|-----------|-----|-----|-----|-------|------------|
| Autumn | | | | | |
| Spring Boot | | | | | |
| Rails | | | | | |
| Django | | | | | |
| Phoenix | | | | | |
| Loco | | | | | |

## Validation Failure Path — Latency (p50 / p95 / p99)

| Framework | p50 | p95 | p99 | 422 rate |
|-----------|-----|-----|-----|---------|
| Autumn | | | | |
| Spring Boot | | | | |
| Rails | | | | |
| Django | | | | |
| Phoenix | | | | |
| Loco | | | | |

## Auth-Protected Route — Latency (p50 / p95 / p99)

| Framework | p50 | p95 | p99 | req/s |
|-----------|-----|-----|-----|-------|
| Autumn | | | | |
| Spring Boot | | | | |
| Rails | | | | |
| Django | | | | |
| Phoenix | | | | |
| Loco | | | | |

## Memory Under Load (during 60 s json-crud run)

| Framework | Peak RSS | Average RSS |
|-----------|----------|-------------|
| Autumn | | |
| Spring Boot | | |
| Rails | | |
| Django | | |
| Phoenix | | |
| Loco | | |

## Build Time

| Framework | `docker compose build` |
|-----------|----------------------|
| Autumn | |
| Spring Boot | |
| Rails | |
| Django | |
| Phoenix | |
| Loco | |

## Notes and Caveats

_Document any deviations from the standard methodology here._
