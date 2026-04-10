# 🔭 Vantage: Spec for WebSocket Support

## 👤 User Story
As a Developer building interactive real-time applications (like chat, live dashboards, or collaborative editing), I want a simple, macro-driven way to define WebSocket endpoints (`#[ws]`), so that I can easily integrate bidirectional, low-latency communication without writing boilerplate connection upgrade logic.

## 💼 The "So What?" (Business Problem)
Real-time features are no longer just "nice-to-have"; they are foundational requirements for many modern web applications. Currently, developers have to step outside the framework's core abstractions and use raw Axum routers to handle WebSocket connections. This breaks the Autumn framework's "convention-over-configuration" promise. By providing a first-class `#[ws]` macro, we retain developers within the Autumn ecosystem, significantly reducing their cognitive load and development time for real-time features. Utility drives adoption, and seamless WebSocket support directly increases the utility of Autumn.

## 🎯 Success Metrics
* **Integration Speed:** A developer can implement a basic echo WebSocket server in under 5 minutes using only Autumn documentation.
* **Consistency:** The `#[ws]` macro feels identical to use as `#[get]` or `#[post]`, utilizing the same parameter extraction and error handling patterns where applicable.
* **Adoption:** 20% of new projects generated with `autumn new` include at least one `#[ws]` endpoint within the first month of the feature release.

## 🔍 Gap Analysis
* **The Market:** Other frameworks like Loco or Rocket offer varying degrees of built-in WebSocket or SSE support. Node.js frameworks (like NestJS) have highly abstracted Gateway paradigms.
* **Our Current State:** We rely on Axum's native WebSocket support under the hood, but we do not expose it ergonomically through our macro system. Users must bypass `routes![]` and use `axum::routing::get` directly for WebSockets.
* **The Gap:** We need a macro (`#[ws("/path")]`) that automatically handles the HTTP upgrade request and provides a clean stream/sink interface to the developer, perfectly aligned with the existing Autumn developer experience.

## ✅ Acceptance Criteria
* The framework must provide a `#[ws("path")]` macro for defining WebSocket routes.
* The macro must automatically handle the HTTP to WebSocket upgrade handshake.
* The handler function must be able to accept standard Autumn extractors (like `State`, `Path`, `Query`).
* The handler function must receive a clean interface for reading messages from and writing messages to the connected client.
* The feature must integrate cleanly with the existing `routes![]` macro and router builder.
* Documentation must include a full working example of a simple chat server or echo server.

## 🚫 Out of Scope
* Defining specific real-time protocols over WebSockets (e.g., implementing Socket.io, STOMP, or GraphQL subscriptions).
* Built-in pub/sub infrastructure or message brokers (like Redis integration for scaling WebSockets across instances) - this belongs in a future distributed architecture epic.
* Automatic fallback to Server-Sent Events (SSE) or long-polling if WebSockets fail.
