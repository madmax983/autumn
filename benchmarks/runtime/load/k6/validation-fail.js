// k6 load test: validation failure path
//
// Sends deliberately invalid POST /api/posts payloads and asserts the
// framework returns 422 Unprocessable Entity. This measures the cost of
// validation + error-response rendering under load.
//
// Scenarios exercised:
//   - blank title  → 422
//   - title too long (300 chars) → 422
//   - blank body   → 422
//   - blank author → 422
//
// Usage:
//   BASE_URL=http://localhost:8001 k6 run load/k6/validation-fail.js
//   BASE_URL=http://localhost:8001 k6 run --vus 30 --duration 30s load/k6/validation-fail.js
//
// Environment variables:
//   BASE_URL (required)
//   VUS      (default 20)
//   DURATION (default 30s)

import http from "k6/http";
import { check, sleep } from "k6";
import { Rate, Trend } from "k6/metrics";

const BASE_URL = __ENV.BASE_URL || "http://localhost:8001";
const PARAMS = {
  headers: { "Content-Type": "application/json" },
  responseCallback: http.expectedStatuses(422),
};

export const options = {
  vus:      parseInt(__ENV.VUS || "20"),
  duration: __ENV.DURATION || "30s",
  thresholds: {
    http_req_failed:          ["rate<0.01"],
    "validation_422_rate":    ["rate>0.99"],
  },
};

const errorRate    = new Rate("errors");
const validRate    = new Rate("validation_422_rate");
const validLatency = new Trend("validation_duration", true);

const INVALID_PAYLOADS = [
  // blank title
  { title: "", body: "Some body text.", published: false, author: "tester" },
  // title too long
  { title: "x".repeat(300), body: "Some body text.", published: false, author: "tester" },
  // blank body
  { title: "A valid title", body: "", published: false, author: "tester" },
  // blank author
  { title: "A valid title", body: "Some body text.", published: false, author: "" },
];

export default function () {
  const payload = INVALID_PAYLOADS[Math.floor(Math.random() * INVALID_PAYLOADS.length)];
  const res = http.post(
    `${BASE_URL}/api/posts`,
    JSON.stringify(payload),
    PARAMS
  );
  validLatency.add(res.timings.duration);

  const is422 = res.status === 422;
  validRate.add(is422);
  check(res, { "validation returns 422": () => is422 });
  errorRate.add(res.status >= 500);

  sleep(0.05);
}
