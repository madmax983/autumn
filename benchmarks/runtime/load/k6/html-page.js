// k6 load test: server-rendered HTML pages
//
// Exercises the HTML rendering paths:
//   GET /posts       (list page)
//   GET /posts/:id   (detail page — picks a random ID from 1..1000)
//
// Usage:
//   BASE_URL=http://localhost:8001 k6 run load/k6/html-page.js
//   BASE_URL=http://localhost:8001 k6 run --vus 50 --duration 60s load/k6/html-page.js
//
// Environment variables:
//   BASE_URL   (required) — target app base URL
//   VUS        (default 20)
//   DURATION   (default 30s)
//   POST_COUNT (default 1000) — seed post count used to randomize IDs

import http from "k6/http";
import { check, sleep } from "k6";
import { Rate, Trend } from "k6/metrics";

const BASE_URL   = __ENV.BASE_URL   || "http://localhost:8001";
const POST_COUNT = parseInt(__ENV.POST_COUNT || "1000");

export const options = {
  vus:      parseInt(__ENV.VUS || "20"),
  duration: __ENV.DURATION || "30s",
  thresholds: {
    http_req_failed:   ["rate<0.01"],
    http_req_duration: ["p(95)<500"],
  },
};

const errorRate  = new Rate("errors");
const listTime   = new Trend("html_list_duration",   true);
const detailTime = new Trend("html_detail_duration", true);

export default function () {
  // --- List page ---
  const listRes = http.get(`${BASE_URL}/posts`);
  listTime.add(listRes.timings.duration);
  const listOk = check(listRes, {
    "html list 200":          (r) => r.status === 200,
    "html list contains h1":  (r) => r.body.includes("<h1>"),
  });
  errorRate.add(!listOk);

  // --- Detail page (random post from seed range) ---
  const id = Math.floor(Math.random() * POST_COUNT) + 1;
  const detailRes = http.get(`${BASE_URL}/posts/${id}`);
  detailTime.add(detailRes.timings.duration);
  const detailOk = check(detailRes, {
    "html detail 200":         (r) => r.status === 200,
    "html detail contains h1": (r) => r.body.includes("<h1>"),
  });
  errorRate.add(!detailOk);

  sleep(0.1);
}
