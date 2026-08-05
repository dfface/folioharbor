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

## Fix Round 1

### Correlated JSON request rejection

Test: `request_validation_failures_are_correlated_problem_details` in `crates/http/tests/auth_routes.rs`.

RED command:

```text
cargo test -p folioharbor-http --test auth_routes request_validation_failures_are_correlated_problem_details -- --exact
```

Relevant raw failure:

```text
assertion `left == right` failed
  left: Some("text/plain; charset=utf-8")
 right: Some("application/problem+json")
test result: FAILED. 0 passed; 1 failed
```

GREEN used the identical command and returned:

```text
test request_validation_failures_are_correlated_problem_details ... ok
test result: ok. 1 passed; 0 failed
```

The test exercises malformed JSON (`400 malformed_json`), a missing required field (`422 invalid_json_body`), and unsupported content type (`415 unsupported_media_type`). Each response is `application/problem+json`, contains a 26-character middleware request ID, and uses the same ID in `instance`.

### Clock-based safe session status

Test: `safe_session_status_uses_clock_for_idle_and_absolute_expiry` in `crates/application/tests/identity_use_cases.rs`.

RED command:

```text
cargo test -p folioharbor-application --test identity_use_cases safe_session_status_uses_clock_for_idle_and_absolute_expiry -- --exact
```

Relevant raw failure:

```text
assertion `left == right` failed
  left: Active
 right: IdleExpired
test result: FAILED. 0 passed; 1 failed
```

After injecting `Clock` into both current/list use cases, GREEN returned:

```text
test safe_session_status_uses_clock_for_idle_and_absolute_expiry ... ok
test result: ok. 1 passed; 0 failed
```

The deterministic boundary fixture proves equality at idle and absolute expiry maps to `IdleExpired` and `AbsolutelyExpired`, respectively.

### Structurally complete OpenAPI contract

Test: `openapi_auth_operations_have_resolved_bodies_success_examples_and_actual_statuses` in `crates/http/tests/openapi_contract.rs`.

RED command:

```text
cargo test -p folioharbor-http --test openapi_contract openapi_auth_operations_have_resolved_bodies_success_examples_and_actual_statuses -- --exact
```

Relevant raw failure:

```text
assertion `left == right` failed: post /api/v1/auth/register
  left: {"202", "422", "429"}
 right: {"202", "400", "413", "415", "422", "429"}
test result: FAILED. 0 passed; 1 failed
```

GREEN used the identical command and returned:

```text
test openapi_auth_operations_have_resolved_bodies_success_examples_and_actual_statuses ... ok
test result: ok. 1 passed; 0 failed
```

The test resolves local request-body, response, and schema references; verifies purpose-specific request schemas/examples; verifies success schemas/examples for body-bearing responses; and checks exact runtime status matrices for all nine routes.

### Atomic password-reset rotation

Application test: `password_reset_issues_fresh_opaque_session_and_csrf_tokens` in `crates/application/tests/identity_use_cases.rs`.

RED command:

```text
cargo test -p folioharbor-application --test identity_use_cases password_reset_issues_fresh_opaque_session_and_csrf_tokens -- --exact
```

Relevant raw failure:

```text
error[E0061]: this function takes 3 arguments but 4 arguments were supplied
error[E0609]: no field `session_token` on type `PasswordResetComplete`
error[E0609]: no field `csrf_token` on type `PasswordResetComplete`
```

GREEN used the identical command and returned:

```text
test password_reset_issues_fresh_opaque_session_and_csrf_tokens ... ok
test result: ok. 1 passed; 0 failed
```

HTTP test: `password_reset_sets_fresh_secure_cookies_without_tokens_in_json` in `crates/http/tests/auth_routes.rs`.

RED command and failure:

```text
cargo test -p folioharbor-http --test auth_routes password_reset_sets_fresh_secure_cookies_without_tokens_in_json -- --exact
assertion `left == right` failed
  left: 204
 right: 200
test result: FAILED. 0 passed; 1 failed
```

GREEN returned:

```text
test password_reset_sets_fresh_secure_cookies_without_tokens_in_json ... ok
test result: ok. 1 passed; 0 failed
```

Real PostgreSQL test: `password_reset_atomically_revokes_old_sessions_and_issues_replacement` in `crates/postgres/tests/password_reset_rotation.rs`.

PostgreSQL 18 setup:

```text
docker run --rm -d --name folioharbor-task5-fix1-pg18 -e POSTGRES_HOST_AUTH_METHOD=trust -p 55435:5432 postgres:18-alpine
docker exec folioharbor-task5-fix1-pg18 pg_isready -U postgres
/var/run/postgresql:5432 - accepting connections
```

The interface-first RED showed the missing atomic input boundary:

```text
error[E0050]: method `reset_password` has 4 parameters but the declaration in trait `reset_password` has 5
```

The behavioral mutation RED removed only the replacement insert and ran:

```text
FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55435/postgres cargo test -p folioharbor-postgres --test password_reset_rotation password_reset_atomically_revokes_old_sessions_and_issues_replacement -- --exact
Error: replacement session was not issued
test result: FAILED. 0 passed; 1 failed
```

Restoring the insert inside the same reset function and running the identical command returned:

```text
test password_reset_atomically_revokes_old_sessions_and_issues_replacement ... ok
test result: ok. 1 passed; 0 failed
```

The function now consumes the reset token, updates the credential, revokes every old session, and inserts the hashed replacement session/CSRF pair in one PostgreSQL transaction. The test proves the old secret no longer authenticates and the replacement does.

### Checked SQL and complete gates

The preparation database was created as owner `folioharbor_owner`; migrations reported:

```text
Applied 1/migrate platform
Applied 2/migrate identity
Applied 3/migrate auth rate limits
Applied 4/migrate password reset rotation
```

Checked-query metadata command:

```text
DATABASE_URL=postgres://folioharbor_api@127.0.0.1:55435/folioharbor_prepare_task5_fix1 cargo sqlx prepare --workspace
Finished `dev` profile
query data written to .sqlx in the workspace root
```

The final metadata check also passed:

```text
DATABASE_URL=postgres://folioharbor_api@127.0.0.1:55435/folioharbor_prepare_task5_fix1 cargo sqlx prepare --workspace --check
Finished `dev` profile
```

Final formatting and diff gates:

```text
cargo fmt --all -- --check
git diff --check
exit 0; no output
```

Final workspace gate:

```text
FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55435/postgres cargo test --workspace
identity_use_cases: 8 passed; 0 failed
auth_routes: 7 passed; 0 failed
openapi_contract: 1 passed; 0 failed
identity_repository: 4 passed; 0 failed
migration_from_zero: 1 passed; 0 failed
password_reset_rotation: 1 passed; 0 failed
rate_limits: 1 passed; 0 failed
all remaining unit, integration, and doc tests: 0 failed
exit 0
```

Final strict lint gate:

```text
cargo clippy --workspace --all-targets --all-features -- -D warnings
Finished `dev` profile
exit 0
```

The first online dependency-policy retry encountered an external GitHub HTTP/2 advisory-fetch failure. The cached advisory database was then checked explicitly without network:

```text
cargo deny --offline check
advisories ok, bans ok, licenses ok, sources ok
exit 0
```

Only the repository's pre-existing duplicate-dependency and unmatched ISC allowance warnings were emitted. The scoped PostgreSQL 18 container was stopped and removed after verification.
