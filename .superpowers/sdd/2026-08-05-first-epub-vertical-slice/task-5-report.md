# Task 5 Report: Secure Local Authentication API

## Outcome

Implemented the `/api/v1/auth` HTTP surface for registration, verification, login, logout, password recovery, current-session lookup, session listing, and owner-scoped revocation. The API uses opaque secure cookies, session-bound double-submit CSRF protection, correlated RFC 9457 problem responses, and durable purpose-specific PostgreSQL token buckets.

## Design and implementation

- Added narrow application use-case traits and an `IdentityApi` facade so Axum state depends on transport-neutral interfaces rather than concrete orchestration types.
- Extended authenticated session principals with only the stored CSRF-token hash required at the transport boundary.
- Added current/list/revoke session use cases and owner-scoped repository operations, including the typed `UserRevoked` reason.
- Added cookie authentication middleware and `AuthenticatedActor`/`MaybeActor` extractors. No Bearer-token parsing is accepted.
- Added constant-time CSRF hash comparison for unsafe authenticated methods. Login emits fresh session and CSRF tokens; logout, password reset, and current-session revocation clear both cookies.
- Added request IDs and configured public problem-instance URLs to every problem response.
- Added per-purpose rate policies keyed by HMAC over normalized identifier plus masked IPv4 `/24` or IPv6 `/64`; neither raw identifiers nor full IP addresses are persisted.
- Added the PostgreSQL bucket migration and a row-locked, transactional refill/consume adapter using compile-time checked `sqlx::query!` statements.
- Added the complete OpenAPI auth contract with cookie security, CSRF header, stable problem codes, examples, and reusable `ProblemDetails` responses.
- Wired the production API composition root to PostgreSQL, the identity facade, durable limiter, system clock/randomness, and peer-address-aware Axum serving. Mail delivery remains the explicit Task 17 boundary.

## TDD evidence

- Route RED: `cargo test -p folioharbor-http --test auth_routes` failed because the planned router, state, and application interfaces did not exist.
- Route GREEN: the same command passed all 5 route-contract tests.
- PostgreSQL RED: with a scoped PostgreSQL 18 instance, `FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55435/postgres cargo test -p folioharbor-postgres --test rate_limits` failed with `rate limit persistence failed` while the durable production behavior was absent.
- PostgreSQL GREEN: the same command passed the 20-request concurrency test with exactly 3 allowed and 17 denied.
- OpenAPI RED: `cargo test -p folioharbor-http --test openapi_contract` failed because an error response did not reference `ProblemDetails`.
- OpenAPI GREEN: the same command passed after all error responses reused the component response/schema.

## Verification

- `cargo fmt --all -- --check`
- `cargo test -p folioharbor-http --test auth_routes --test openapi_contract`
- `cargo test -p folioharbor-http auth::tests::masks_client_addresses_before_rate_limit_keying`
- `FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55435/postgres cargo test --workspace`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo deny check`
- `DATABASE_URL=postgres://folioharbor_api@127.0.0.1:55435/folioharbor_prepare_task5 cargo sqlx prepare --workspace`

All gates passed. SQLx offline metadata was regenerated for the checked queries.

## Scope notes

The brief required functional session routes but the prior application surface did not expose them, so the implementation minimally extended the application and identity repository contracts. Workspace dependency manifests, the domain revocation enum, and SQLx offline metadata also changed as necessary to compile and verify the requested feature.
