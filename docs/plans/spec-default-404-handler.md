# Spec: Default 404 Error Handler

## What business problem does this solve?
Currently, when users hit an undefined route, Autumn returns an empty 404 response (`content-length: 0`). This causes a poor developer experience as users might assume the server crashed or their request failed silently. A default error page provides immediate, actionable feedback that the framework is running but the route is not found, saving time debugging.

## User Story
As a Developer, I want a default 404 error page (HTML or JSON) instead of a blank response, so that I can easily recognize when I have requested an undefined route.

## Acceptance Criteria
- Must return an HTTP 404 status code.
- Must return a default HTML error page if the `Accept` header prefers HTML.
- Must return a default JSON error object if the `Accept` header prefers JSON.
- Must not return an empty response body (`content-length: 0`) for unmatched routes.

## Out of Scope
- Custom error page configuration (Phase 2).
- Default pages for other 4xx/5xx errors (unless handled universally by the same mechanism).
