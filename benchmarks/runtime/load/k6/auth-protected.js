// k6 load test: authenticated/protected route
//
// Tests GET /api/posts/protected with three scenarios:
//   1. Valid Bearer token (seed token) → 200
//   2. Missing Authorization header   → 401
//   3. Invalid token                  → 401
//
// Usage:
//   BASE_URL=http://localhost:8001 k6 run load/k6/auth-protected.js
//   BASE_URL=http://localhost:8001 \
//     BENCH_TOKEN=benchmark-token-abc123 \
//     k6 run --vus 20 --duration 30s load/k6/auth-protected.js
//
// Environment variables:
//   BASE_URL    (required)
//   BENCH_TOKEN (default "benchmark-token-abc123") — seed token from seed.sql
//   VUS         (default 20)
//   DURATION    (default 30s)

import http from "k6/http";
import { check, sleep } from "k6";
import { Rate, Trend } from "k6/metrics";

const BASE_URL    = __ENV.BASE_URL    || "http://localhost:8001";
const BENCH_TOKEN = __ENV.BENCH_TOKEN || "benchmark-token-abc123";
const PROTECTED   = `${BASE_URL}/api/posts/protected`;
const AUTH_FAILURE_PARAMS = {
  responseCallback: http.expectedStatuses(401),
};

export const options = {
  vus:      parseInt(__ENV.VUS || "20"),
  duration: __ENV.DURATION || "30s",
  thresholds: {
    http_req_failed:   ["rate<0.01"],
    http_req_duration: ["p(95)<400"],
  },
};

const errorRate   = new Rate("errors");
const authedTime  = new Trend("auth_success_duration", true);
const unauthTime  = new Trend("auth_failure_duration", true);

export default function () {
  const scenario = Math.random();

  if (scenario < 0.7) {
    // 70 %: valid token
    const res = http.get(PROTECTED, {
      headers: { Authorization: `Bearer ${BENCH_TOKEN}` },
    });
    authedTime.add(res.timings.duration);
    check(res, { "valid token → 200": (r) => r.status === 200 });
    errorRate.add(res.status >= 500);

  } else if (scenario < 0.85) {
    // 15 %: no Authorization header
    const res = http.get(PROTECTED, AUTH_FAILURE_PARAMS);
    unauthTime.add(res.timings.duration);
    check(res, { "no token → 401": (r) => r.status === 401 });
    errorRate.add(res.status >= 500);

  } else {
    // 15 %: wrong token
    const res = http.get(PROTECTED, {
      headers: { Authorization: "Bearer invalid-token-xyz" },
      responseCallback: http.expectedStatuses(401),
    });
    unauthTime.add(res.timings.duration);
    check(res, { "bad token → 401": (r) => r.status === 401 });
    errorRate.add(res.status >= 500);
  }

  sleep(0.05);
}
