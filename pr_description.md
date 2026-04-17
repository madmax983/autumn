🔒 Warden: [security fix] Add harvest_api_with_auth for management API protection

This PR fixes the unauthenticated access vulnerability by introducing `harvest_api_with_auth` to the `HarvestExt` trait, allowing developers to secure the management API using custom middleware.

We also expanded test coverage to cover the new `harvest_api_with_auth` logic, hitting the `api_middleware` configurations to satisfy Codecov patch check.
