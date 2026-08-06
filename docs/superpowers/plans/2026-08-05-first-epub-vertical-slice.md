# FolioHarbor First EPUB Vertical Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the approved two-user EPUB vertical slice from an empty PostgreSQL database through registration, shared-library authorization, EPUB import, browser reading, progress synchronization, and permission-controlled download.

**Architecture:** Implement a modular monolith in one Rust workspace with pure domain and application crates, PostgreSQL/local-storage/EPUB adapters, and thin API, Worker, and CLI processes. A React TypeScript web client consumes the generated OpenAPI contract; PostgreSQL RLS remains the final tenant boundary and a durable PostgreSQL job queue coordinates imports and email.

**Tech Stack:** Rust 1.88 / edition 2024, Tokio, Axum, SQLx with PostgreSQL 18.x, Serde, Argon2id, tracing/OpenTelemetry, React 19, TypeScript strict mode, Vite, TanStack Query, Vitest, Playwright, pnpm 11, Docker Compose.

**Source specification:** [`../specs/2026-08-05-first-epub-vertical-slice-design.md`](../specs/2026-08-05-first-epub-vertical-slice-design.md)

## Global Constraints

- PostgreSQL 18.x is the only supported relational database; use `PgPool`, never `AnyPool` or SQLite test substitutes.
- All schema changes are ordered SQLx migrations. API and Worker check schema compatibility but never generate or silently apply schema changes.
- The domain crate must not depend on Axum, SQLx, SMTP, ZIP, filesystem, or JSON transport types.
- The application crate owns use-case interfaces and ports; adapters depend inward. No adapter may call another adapter directly.
- Production Rust forbids `unsafe`; avoid `unwrap`, `expect`, `panic!`, boolean-parameter APIs, generic `Manager`/`Helper`/`Utils` names, and stringly typed IDs or states.
- Keep one abstraction level per function, prefer domain vocabulary, make invalid states unrepresentable, and explain reasons rather than mechanics in comments.
- A source file should normally stay below 300 nonblank lines. Split by responsibility when a file has more than one reason to change; do not split cohesive code to satisfy a number mechanically.
- Every behavior change begins with a failing focused test, then the smallest implementation, then focused and broader verification, then a commit.
- Rust quality gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features`.
- TypeScript quality gate: strict mode, no application `any`, ESLint with zero warnings, component tests, and Playwright E2E.
- Tests use behavior names and observable outcomes. Mock only ports at process boundaries; database and RLS tests use real PostgreSQL 18.
- Browser authentication uses an opaque `HttpOnly; Secure; SameSite=Lax` cookie plus a session-bound CSRF header. Do not use JWT browser sessions.
- Never expose database errors, filesystem paths, Blob keys, ZIP entry paths, token plaintext, or cross-library deduplication timing/results.
- Reader content is untrusted: no script, event handlers, forms, iframes, objects, meta refresh, or external network resources.
- Default configuration remains: public registration on, email verification on, personal library provisioning on, reader download off, 5 GiB library quota, 1 GiB upload limit, 1 GiB free-space reserve, `storage.dedup_scope = "instance"`, 24-hour failed-upload retention, 24-hour Blob GC delay, and 7-day Item recovery.
- UI copy ships in Simplified Chinese and English, meets the approved WCAG 2.2 AA baseline, and never changes stable API codes based on locale.
- Do not implement OIDC, S3, TXT/PDF/comics/audio, Meilisearch, annotations, public catalog/content, ActivityPub/OPDS, custom roles, generic ACLs, Android, or legacy migration in this plan.

## Execution Rules

1. Execute tasks in order; a task may consume only interfaces listed from prior tasks.
2. Before each task, reread its files and interfaces. Do not pre-build later abstractions.
3. Use a dedicated worktree when execution starts; do not implement directly on `main`.
4. After each task, request a requirements review and then a code-quality review before moving on.
5. If an interface must change, update this plan and all later references in the same documentation commit before coding against the new name.
6. Keep commits limited to the task. Never mix opportunistic refactors with feature behavior.

## Planned Repository Shape

```text
.
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── rustfmt.toml
├── deny.toml
├── .cargo/config.toml
├── .github/workflows/ci.yml
├── apps/
│   ├── api/src/main.rs
│   ├── worker/src/main.rs
│   └── cli/src/main.rs
├── crates/
│   ├── domain/src/{identity/,libraries/,catalog/,imports/,reader/}
│   ├── application/src/{ports/,identity/,libraries/,catalog/,imports/,reader/}
│   ├── postgres/src/{context,identity,libraries,catalog,imports,reader,audit}.rs
│   ├── storage-local/src/{lib.rs,paths.rs,capacity.rs}
│   ├── epub/src/{lib.rs,archive.rs,package.rs,sanitize.rs}
│   ├── http/src/{lib.rs,problem.rs,auth.rs,routes/,middleware/}
│   └── test-support/src/{lib.rs,postgres.rs,fixtures.rs,clock.rs}
├── migrations/
├── openapi/folioharbor-v1.yaml
├── deploy/
│   ├── compose.yaml
│   ├── postgres/init/001-roles.sql
│   └── example.folioharbor.toml
├── web/
│   ├── src/{app,api,features,i18n,test}/
│   ├── e2e/
│   └── package.json
└── tests/fixtures/epub/
```

The directory map is a dependency contract, not permission to create empty modules. A task creates a file only when it adds tested behavior.

## Stable Cross-Task Interfaces

These names are fixed unless the plan itself is amended:

```rust
// crates/domain/src/id.rs
pub struct UserId(Uuid);
pub struct SessionId(Uuid);
pub struct LibraryId(Uuid);
pub struct ManifestationId(Uuid);
pub struct ItemId(Uuid);
pub struct BlobId(Uuid);
pub struct UploadId(Uuid);
pub struct JobId(Uuid);
pub struct DeviceId(Uuid);
pub struct RequestId(Ulid);
pub struct ErrorId(Ulid);

// crates/application/src/actor.rs
pub struct Actor {
    pub user_id: UserId,
    pub session_id: SessionId,
}

pub struct RequestContext {
    pub actor: Actor,
    pub request_id: RequestId,
}

// crates/application/src/error.rs
pub struct FieldViolation {
    pub field: &'static str,
    pub code: &'static str,
}

pub enum AppError {
    Unauthenticated,
    Forbidden { code: &'static str },
    NotFound { code: &'static str },
    Conflict { code: &'static str },
    Invalid { code: &'static str, fields: Vec<FieldViolation> },
    PayloadTooLarge,
    RateLimited { retry_after: Duration },
    StorageExhausted,
    DependencyUnavailable { code: &'static str },
    Internal { error_id: ErrorId },
}
```

UUID-backed domain IDs expose `new()`, `from_uuid(Uuid)`, and `as_uuid()`; request/error correlation IDs expose equivalent ULID constructors. None implements `Deref` to its primitive. Secrets use `secrecy::SecretString` or private byte wrappers and redact `Debug`.

Application ports expose use-case-shaped atomic operations rather than generic CRUD or a database-flavored unit of work. Every mutating port method documents its transaction boundary and accepts the authorization/audit facts it must revalidate and persist atomically. PostgreSQL adapters own SQL transactions internally; application/domain code never receives `sqlx::Transaction`, and one port must not start a hidden nested transaction inside another port call.

---

### Task 1: Establish the Workspace and Executable Quality Gates

**Files:**

- Create: `rust-toolchain.toml`
- Create: `Cargo.toml`
- Create: `.cargo/config.toml`
- Create: `rustfmt.toml`
- Create: `deny.toml`
- Create: `crates/domain/Cargo.toml`
- Create: `crates/domain/src/lib.rs`
- Create: `crates/domain/src/id.rs`
- Create: `crates/domain/tests/id_contract.rs`
- Create: `apps/api/Cargo.toml`
- Create: `apps/api/src/main.rs`
- Create: `apps/worker/Cargo.toml`
- Create: `apps/worker/src/main.rs`
- Create: `apps/cli/Cargo.toml`
- Create: `apps/cli/src/main.rs`
- Create: `.github/workflows/ci.yml`
- Modify: `README.md`

**Interfaces:**

- Produces the typed IDs shown in “Stable Cross-Task Interfaces”.
- Produces the standard Cargo workspace commands used by every later task; no custom xtask is created in this task.

- [ ] **Step 1: Install and verify the pinned toolchain**

Install Rust through the official rustup distribution if it is absent. Create `rust-toolchain.toml` with channel `1.88.0`, profile `minimal`, and components `rustfmt` and `clippy`; install `cargo-deny` with a lockfile-pinned compatible release. Run `rustup show active-toolchain`, `cargo --version`, and `cargo deny --version`. Expected: Rust 1.88 and cargo-deny are available. If 1.88 cannot compile a selected dependency, change the pin in a dedicated plan amendment with evidence; do not silently float to `stable`.

- [ ] **Step 2: Write the failing ID contract test**

```rust
use folioharbor_domain::id::{LibraryId, UserId};

#[test]
fn ids_of_different_domains_are_not_interchangeable_and_round_trip() {
    let raw = uuid::Uuid::now_v7();
    let user = UserId::from_uuid(raw);
    let library = LibraryId::from_uuid(raw);

    assert_eq!(user.as_uuid(), raw);
    assert_eq!(library.as_uuid(), raw);
    assert_ne!(format!("{user:?}"), format!("{library:?}"));
}
```

Run `cargo test -p folioharbor-domain --test id_contract`. Expected: FAIL because the workspace and ID types do not exist.

- [ ] **Step 3: Create the minimal workspace and typed IDs**

Use resolver `2`, edition `2024`, `rust-version = "1.88"`, `#![forbid(unsafe_code)]`, and shared workspace dependencies. Define IDs with a private `Uuid` field and a small macro private to `id.rs`; export concrete types, not the macro. Each binary initially returns `anyhow::Result<()>` and prints only its process name through `tracing`, proving the workspace links without inventing application behavior.

- [ ] **Step 4: Add quality and dependency policy**

Configure Clippy workspace lints for `all`, `pedantic`, and `unwrap_used`; allow individual noisy lints only beside a written reason. Configure `cargo-deny` to reject known vulnerabilities, unknown registries, and copyleft licenses incompatible with the intended project license. CI runs formatting, Clippy, tests, `cargo deny check`, and a PostgreSQL 18 service placeholder that later tasks consume.

- [ ] **Step 5: Verify and commit**

Run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

Expected: all commands exit 0. Commit:

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml deny.toml .cargo .github apps crates README.md
git commit -m "build: establish Rust workspace quality gates"
```

### Task 2: Add Configuration, Time, Randomness, and Problem Contracts

**Files:**

- Create: `crates/domain/src/time.rs`
- Create: `crates/application/Cargo.toml`
- Create: `crates/application/src/lib.rs`
- Create: `crates/application/src/{actor,error,config}.rs`
- Create: `crates/application/src/ports/{mod,clock,random}.rs`
- Create: `crates/application/tests/config_contract.rs`
- Create: `crates/http/Cargo.toml`
- Create: `crates/http/src/{lib,problem}.rs`
- Create: `crates/http/tests/problem_contract.rs`
- Create: `deploy/example.folioharbor.toml`

**Interfaces:**

- Produces `Clock::now() -> OffsetDateTime` and `RandomSource::fill(&mut [u8])`.
- Produces `Settings::load(ConfigSources) -> Result<Settings, ConfigError>` with nested `server`, `database`, `storage`, `auth`, `mail`, `worker`, and `observability` values, plus a keyed 32-byte-or-stronger application secret ring loaded only from secret files/environment.
- Produces `ProblemDetails::from_app_error(&AppError, &ProblemContext)` and the stable `AppError` variants above.

- [ ] **Step 1: Write failing configuration-default tests**

Assert the approved defaults exactly: registration and email verification true, personal library true, reader download false, quota `5 * 1024^3`, upload limit `1024^3`, free reserve `1024^3`, dedup scope `Instance`, failed retention 24 hours, GC delay 24 hours, recovery 7 days. Also assert environment keys override TOML and CLI values override environment without logging secret values.

Run `cargo test -p folioharbor-application --test config_contract`. Expected: FAIL because `Settings` does not exist.

- [ ] **Step 2: Implement typed configuration**

Use enums for `DedupScope` and URL types, `ByteSize`/`Duration` newtypes for validated values, and `SecretString` for credentials. Load a current application secret with a stable key ID for keyed rate-limit identifiers and encrypted outbox token payloads; reject short/default secrets and support old decryption-only keys for rotation. `Settings::load` must reject an enabled verification/invitation/reset flow without SMTP and reject storage roots that are relative, the filesystem root, or equal to the staging root. Keep parsing separate from validation so error messages name the invalid key.

- [ ] **Step 3: Write the failing RFC 9457 mapping test**

```rust
#[test]
fn quota_conflict_has_stable_problem_shape() {
    let problem = ProblemDetails::from_app_error(
        &AppError::Conflict { code: "quota_exceeded" },
        &ProblemContext::example("01JREQ"),
    );
    assert_eq!(problem.status, 409);
    assert_eq!(problem.code, "quota_exceeded");
    assert_eq!(problem.type_uri.as_str(), "https://library.example/problems/quota-exceeded");
    assert_eq!(problem.request_id, "01JREQ");
}
```

Expected first run: FAIL because the mapping is absent.

- [ ] **Step 4: Implement errors without leaking internals**

Map every `AppError` variant to one stable status/code/title. Keep internal error sources in tracing spans keyed by `error_id`; never serialize sources, SQL, paths, hashes, stack traces, or tenant identifiers. Add a route-independent serializer with content type `application/problem+json`.

- [ ] **Step 5: Verify and commit**

Run focused tests, then the workspace quality gate. Commit:

```bash
git add crates/application crates/domain/src/time.rs crates/http deploy/example.folioharbor.toml
git commit -m "feat: define configuration and problem contracts"
```

### Task 3: Establish PostgreSQL Roles, Migration Runner, and RLS Test Harness

**Files:**

- Create: `deploy/postgres/init/001-roles.sql`
- Create: `migrations/0001_platform.sql`
- Create: `crates/postgres/Cargo.toml`
- Create: `crates/postgres/src/{lib,pool,context,migrate}.rs`
- Create: `crates/test-support/Cargo.toml`
- Create: `crates/test-support/src/{lib,postgres,clock,random}.rs`
- Create: `crates/postgres/tests/{migration_from_zero,transaction_context}.rs`
- Modify: `apps/cli/Cargo.toml`
- Modify: `apps/cli/src/main.rs`

**Interfaces:**

- Produces `PgPools { owner, api, worker }` for tests and process-specific `connect_api`/`connect_worker` constructors for production.
- Produces `PgTransactionContext::apply(&mut PgConnection, &DatabaseContext)` using transaction-local settings only.
- Produces CLI command `folioharbor migrate` with a PostgreSQL advisory lock.

- [ ] **Step 1: Write failing migration and role tests**

The tests create a unique PostgreSQL database, run all migrations as `folioharbor_owner`, verify `folioharbor_api` and `folioharbor_worker` are neither owners nor `BYPASSRLS`, run migrations a second time, and assert no duplicate application. `transaction_context` must prove a pooled connection does not retain `app.user_id`, `app.library_id`, or `app.request_id` after commit and checkout.

Run `cargo test -p folioharbor-postgres --test migration_from_zero --test transaction_context`. Expected: FAIL because roles/migrations do not exist.

- [ ] **Step 2: Create least-privilege database roles**

`001-roles.sql` creates login roles only when absent, gives schema ownership solely to `folioharbor_owner`, and never grants `BYPASSRLS`, superuser, role creation, or database creation to runtime roles. Passwords arrive through deployment secrets, not the SQL file. The official development bootstrap may set local-only passwords through environment substitution in Compose, never commit them.

- [ ] **Step 3: Create platform migration primitives**

`0001_platform.sql` creates the `folioharbor` schema, `schema_metadata`, and stable SQL functions:

```sql
folioharbor.current_user_id() returns uuid
folioharbor.current_library_id() returns uuid
folioharbor.current_request_id() returns text
folioharbor.is_worker() returns boolean
```

Each reads `current_setting(..., true)` safely with `NULLIF`, is `STABLE`, has a fixed empty `search_path`, and is owned by `folioharbor_owner`. Runtime roles receive only required `USAGE`/`EXECUTE` grants.

- [ ] **Step 4: Implement transaction-scoped context and migration CLI**

Apply context only after `BEGIN` with parameterized `SELECT set_config(name, value, true)`. Never interpolate IDs into SQL and never use session-level `SET`. The CLI connects with owner credentials, takes one fixed advisory lock, runs embedded SQLx migrations, reports version numbers, and exits nonzero on a dirty or newer schema.

- [ ] **Step 5: Verify and commit**

Run both focused tests twice to catch leaked database state, then the workspace gate. Commit:

```bash
git add deploy/postgres migrations crates/postgres crates/test-support apps/cli
git commit -m "feat: add versioned PostgreSQL migration foundation"
```

### Task 4: Implement Local Accounts, Passwords, Verification, and Sessions

**Files:**

- Create: `migrations/0002_identity.sql`
- Create: `crates/domain/src/identity/{mod,email,session,token}.rs`
- Modify: `crates/domain/src/lib.rs`
- Create: `crates/application/src/identity/{mod,register,verify,login,logout,reset_password}.rs`
- Create: `crates/application/src/ports/{identity_repository,password_hasher,mailer}.rs`
- Create: `crates/postgres/src/identity.rs`
- Create: `crates/postgres/tests/identity_repository.rs`
- Create: `crates/application/tests/identity_use_cases.rs`

**Interfaces:**

- Produces `RegisterAccount::execute(RegisterAccountCommand) -> Result<PendingAccount, AppError>`.
- Produces `VerifyEmail::execute(VerifyEmailCommand) -> Result<VerifiedAccount, AppError>`.
- Produces `Login::execute(LoginCommand) -> Result<IssuedSession, AppError>` where the plaintext session and CSRF tokens exist only in the return value.
- Produces use-case-shaped repository operations whose mutation methods each execute one documented atomic transaction.

- [ ] **Step 1: Write failing domain and use-case tests**

Cover Unicode trimming plus ASCII-domain normalization, instance-wide normalized-email uniqueness, non-enumerating registration/login/reset responses, Argon2id verification, single-use expiring tokens, verification-required login, idle/absolute session expiry, password-change session revocation, and idempotent logout. Use `FakeClock`, deterministic `RandomSource`, and an in-memory fake implementing only `IdentityRepository`.

Run `cargo test -p folioharbor-application --test identity_use_cases`. Expected: FAIL because use cases and ports do not exist.

- [ ] **Step 2: Implement the identity domain**

Use `NormalizedEmail` with one constructor, token wrappers that expose only `hash_for_storage()`, and explicit `AccountStatus`/`SessionStatus` enums. Hash passwords with Argon2id at no less than 19 MiB, 2 iterations, parallelism 1, a random salt, and a versioned PHC string. Compare token hashes in constant time. Never place plaintext tokens in domain events, logs, database structs, or errors.

- [ ] **Step 3: Create identity tables and RLS/grants**

Create `user_accounts`, `password_credentials`, `user_sessions`, `email_verification_tokens`, `password_reset_tokens`, and `user_devices`. Store UUIDv7 IDs, normalized/display email separately, token hashes as `bytea`, explicit timestamps, revocation reason, and optimistic version where state changes. Add unique constraints for normalized email and token hashes. Account bootstrap queries are instance-scoped and granted through narrow repository SQL; session/device rows use `user_id` RLS where authenticated access is applicable.

- [ ] **Step 4: Implement SQLx repositories and transactional use cases**

Use compile-time checked SQL (`query!`/`query_as!`) with prepared metadata in CI. Registration always returns the same public result whether an account exists. Login performs a dummy Argon2 verification for unknown emails. Token consumption uses one `UPDATE ... WHERE consumed_at IS NULL AND expires_at > now RETURNING` statement. Session lookup stores and queries SHA-256 hashes of 256-bit random opaque tokens.

- [ ] **Step 5: Prove persistence and commit**

Run domain/use-case tests and real-PostgreSQL repository tests, including concurrent token consumption where exactly one caller succeeds. Run the workspace gate. Commit:

```bash
git add migrations/0002_identity.sql crates/domain crates/application crates/postgres
git commit -m "feat: add secure local identity and sessions"
```

### Task 5: Add HTTP Authentication, Cookies, CSRF, and Rate Limits

**Files:**

- Create: `migrations/0003_auth_rate_limits.sql`
- Create: `migrations/0004_password_reset_rotation.sql`
- Create: `crates/application/src/identity/rate_limit.rs`
- Create: `crates/application/src/ports/rate_limit_repository.rs`
- Create: `crates/postgres/src/rate_limits.rs`
- Create: `crates/http/src/auth.rs`
- Create: `crates/http/src/middleware/{mod,request_id,csrf}.rs`
- Create: `crates/http/src/routes/{mod,auth.rs}`
- Create: `crates/http/tests/auth_routes.rs`
- Modify: `apps/api/Cargo.toml`
- Modify: `apps/api/src/main.rs`
- Create: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces Axum extractor `AuthenticatedActor` and optional `MaybeActor`.
- Produces routes under `/api/v1/auth`: register, verify-email, login, logout, forgot-password, reset-password, session, sessions, and revoke-session.
- Produces request header contract `X-CSRF-Token` for unsafe methods.

- [ ] **Step 1: Write failing route contract tests**

Exercise the router in-process. Assert login sets an opaque cookie with `HttpOnly`, `Secure`, `SameSite=Lax`, and scoped `Path=/`; the token is absent from JSON. Assert unsafe authenticated calls without the session-bound CSRF token return RFC 9457 `403 csrf_failed`; safe methods do not require it. Assert unknown/existing email recovery responses are indistinguishable and rate limits return `429` with `Retry-After`.

Run `cargo test -p folioharbor-http --test auth_routes`. Expected: FAIL because routes do not exist.

- [ ] **Step 2: Implement actor extraction and CSRF**

Hash the cookie token before repository lookup, reject expired/revoked sessions, and attach `Actor` without exposing transport details to application use cases. Rotate both session and CSRF token on login, password change, and future privilege elevation. CSRF validation compares the header hash with the session record in constant time. Accept Bearer parsing nowhere in this task; preserve a transport-neutral `Actor` boundary only.

- [ ] **Step 3: Implement durable purpose-specific rate limits**

Create `auth_rate_limit_buckets` keyed by a server-HMAC of purpose plus normalized identifier/IP prefix. Use one transaction and row lock to refill/consume a token bucket. Configure separate policies for registration, login, verification, invitation, and password reset. Never store raw attempted email or full IP in this table.

- [ ] **Step 4: Define and validate the OpenAPI auth contract**

Describe schemas, cookie auth, CSRF header, stable problem codes, examples, and all response statuses in `folioharbor-v1.yaml`. Add a test that parses the document and asserts every registered auth route and every response with an error references `ProblemDetails`.

- [ ] **Step 5: Verify and commit**

Run route tests, PostgreSQL rate-limit concurrency tests, OpenAPI parsing, and the workspace gate. Commit:

```bash
git add migrations/0003_auth_rate_limits.sql migrations/0004_password_reset_rotation.sql crates/application crates/postgres crates/http apps/api openapi
git commit -m "feat: expose secure local authentication API"
```

### Task 6: Implement Libraries, Memberships, Roles, and Invitations

**Files:**

- Create: `migrations/0005_libraries.sql`
- Create: `migrations/0006_builtin_roles.sql`
- Create: `crates/domain/src/libraries/{mod,role,membership,invitation}.rs`
- Create: `crates/application/src/libraries/{mod,provision_personal,invite_member,accept_invitation,change_role,remove_member,settings}.rs`
- Create: `crates/application/src/ports/library_repository.rs`
- Create: `crates/postgres/src/libraries.rs`
- Create: `crates/application/tests/library_use_cases.rs`
- Create: `crates/postgres/tests/library_repository.rs`
- Modify: `crates/application/src/identity/login.rs`
- Modify: `crates/http/tests/auth_routes.rs`

**Interfaces:**

- Produces permissions `library.manage`, `member.invite`, `holding.view`, `holding.edit`, `item.read`, and `item.download` as stable codes.
- Produces `ProvisionPersonalLibrary`, `InviteMember`, `AcceptInvitation`, `ChangeMemberRole`, `RemoveMember`, and `UpdateLibrarySettings` use cases.
- Produces membership and permission facts consumed by Task 7's `Authorization` service.

- [ ] **Step 1: Write failing business-rule tests**

Cover idempotent one-personal-library provisioning, invited users retaining their personal library, invitation email binding/expiry/single use, three immutable built-in roles, owner-only membership management, editor/reader denials, and the invariant that a library always retains one active owner. Include two concurrent attempts to remove/demote the final owners; at most one may commit.

- [ ] **Step 2: Implement explicit library commands**

Use one command type per use case rather than optional fields. Do not expose a generic “update membership” operation. `RoleCode` accepts only seeded roles in this slice. Invitation acceptance compares the authenticated account’s normalized email with the invitation binding and consumes the token in the membership transaction.

- [ ] **Step 3: Create library schema and seed roles**

Create `libraries`, `library_memberships`, `library_invitations`, `roles`, `permissions`, and `role_permissions`. Use a partial unique index for one personal library per owner, unique active membership per `(library_id,user_id)`, check nonnegative `quota_used_bytes`/`quota_reserved_bytes`, and foreign keys that prevent orphan roles. Seed stable role/permission mappings idempotently in a migration, not at API startup.

- [ ] **Step 4: Implement repositories with concurrency protection**

Lock the library row before owner-count changes and recheck the invariant in the same transaction. Consume invitations atomically. Provision personal library and owner membership in one transaction keyed by `personal_owner_id`, returning the existing row on retries.

After credential and verification checks but before issuing a successful login session, invoke personal-library provisioning when enabled. If provisioning fails, return a dependency error and do not issue the session; retrying login safely reuses any already-created personal library. Extend auth-route tests to assert local and invited accounts follow the same rule while preserving a transport-neutral identity boundary for future OIDC.

- [ ] **Step 5: Verify and commit**

Run focused tests, real-PostgreSQL race tests, and the workspace gate. Commit:

```bash
git add migrations/0005_libraries.sql migrations/0006_builtin_roles.sql crates/domain crates/application crates/postgres
git commit -m "feat: add collaborative libraries and memberships"
```

### Task 7: Enforce Authorization, RLS, and Append-Only Audit

**Files:**

- Create: `migrations/0007_audit_and_library_rls.sql`
- Create: `crates/application/src/{authorization,audit}.rs`
- Create: `crates/application/src/ports/{authorization_repository,audit_repository}.rs`
- Create: `crates/postgres/src/{authorization,audit}.rs`
- Create: `crates/postgres/tests/{rls_matrix,audit_append_only}.rs`
- Create: `crates/http/src/routes/libraries.rs`
- Create: `crates/http/tests/library_routes.rs`
- Modify: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces `Authorization::require(actor, action, resource) -> Result<AuthorizationGrant, AppError>` where the grant carries the action/resource facts a mutating repository must revalidate.
- Produces `AuditSink::record_denial(AuditEvent)` for denied requests; successful state changes pass an `AuditEvent` into the mutating repository and persist it in the same transaction.
- Produces library list/detail/settings/member/invitation routes under `/api/v1/libraries`.

- [ ] **Step 1: Write the failing RLS matrix**

Create Alice, Bob, an unrelated user, and owner/editor/reader memberships in real PostgreSQL. For every library-owned table, test no context, wrong library, correct library, API role, Worker role, and connection reuse. Assert no-context/wrong-library reads return zero rows or the documented denial and writes fail. Assert runtime roles cannot disable RLS, alter audit rows, or call owner-only functions.

- [ ] **Step 2: Implement the application authorization boundary**

Resolve stable action codes through role permissions; do not switch directly on `owner/editor/reader` inside feature use cases. Return an `AuthorizationGrant` containing actor, library, action, resource, and observed membership version. Mutating repository methods revalidate that grant while locked and write their success audit in the same transaction. Return 404 for resources the actor may not discover and 403 only for visible resources with a denied action. Use a `ResourceRef` enum rather than polymorphic string IDs.

- [ ] **Step 3: Add forced RLS and audit immutability**

Enable and `FORCE ROW LEVEL SECURITY` for library memberships, invitations, and every library-owned table introduced now or later. Policies depend on transaction-local context and membership, not client-supplied filters. Create partition-ready `audit_events` with actor, effective actor, library, action, resource type/ID, decision, reason code, request/job ID, source, timestamp, and minimized network HMAC. Grant runtime roles INSERT/SELECT where required but never UPDATE/DELETE.

- [ ] **Step 4: Expose library routes through the same authorization service**

Handlers parse transport data, invoke one use case, and map results; they contain no permission branching or SQL. Record invitations, role changes, removals, settings changes, and denials. Update OpenAPI and add route tests that prove reader/editor/owner behavior and anti-enumeration.

- [ ] **Step 5: Verify and commit**

Run the full RLS matrix twice with different pooled connection order, audit tests, route tests, and workspace gate. Commit:

```bash
git add migrations/0007_audit_and_library_rls.sql crates/application crates/postgres crates/http openapi
git commit -m "feat: enforce library authorization and audit"
```

### Task 8: Implement Local Blob Storage, Capacity Protection, and Quotas

**Files:**

- Create: `migrations/0008_storage.sql`
- Create: `crates/domain/src/imports/{mod,blob,quota}.rs`
- Create: `crates/application/src/ports/{blob_store,quota_repository}.rs`
- Create: `crates/storage-local/Cargo.toml`
- Create: `crates/storage-local/src/{lib,paths,capacity}.rs`
- Create: `crates/storage-local/tests/storage_contract.rs`
- Create: `crates/postgres/src/storage.rs`
- Create: `crates/postgres/tests/quota_concurrency.rs`

**Interfaces:**

- Produces `BlobStore::{create_staging, append, read_range, promote, delete, free_bytes}` with opaque `StorageKey`; paths never cross the port.
- Produces `QuotaRepository::{reserve, resize_reservation, consume, release}` with checked byte newtypes.
- Produces `BlobIdentity { namespace, sha256, byte_size }` and `DedupScope::{Instance,Library,Disabled}`.

- [ ] **Step 1: Write failing storage contract tests**

Use a temporary directory. Assert unpredictable staging keys, traversal rejection, bounded streaming append, range reads, atomic/idempotent promotion, hash-preserving reads, idempotent deletion, and refusal when free bytes would fall below 1 GiB (inject a fake capacity probe rather than filling disk).

- [ ] **Step 2: Implement the storage port and local adapter**

The adapter owns path derivation. Derive final object paths from namespace and hash with fixed fan-out; callers see only `StorageKey`. Create with restrictive permissions, fsync the file and parent directory before marking promotion successful, and treat an existing matching destination as success. Never follow symlinks inside the configured roots.

- [ ] **Step 3: Create storage metadata and quota tables**

Create `blobs`, `blob_locations`, and `quota_reservations`. Blob uniqueness is `(storage_namespace, sha256, byte_size)`. Resolve namespace exactly as follows: one stable instance namespace for `instance`, a stable Library-derived namespace for `library`, and a fresh upload-derived namespace for `disabled`; changing configuration affects new writes only. Locations have explicit `staging`, `ready`, `quarantined`, and `purged` states. Quota reservations carry library, upload, reserved bytes, expiry, and state; library counters remain nonnegative under check constraints.

- [ ] **Step 4: Implement quota transactions and race tests**

Lock one library row for reserve/resize/consume/release. Reject when `used + reserved + requested > quota`. Physical free-space protection is checked before staging and again before promotion. Cross-library dedup never changes logical usage. Concurrent reservations may not exceed quota.

- [ ] **Step 5: Verify and commit**

Run storage contracts on local filesystems, all three dedup-scope namespace tests, PostgreSQL quota races, RLS tests for quota reservations, and workspace gate. Commit:

```bash
git add migrations/0008_storage.sql crates/domain crates/application crates/storage-local crates/postgres
git commit -m "feat: add quota-aware local blob storage"
```

### Task 9: Add Upload Sessions and the Durable Job Queue

**Files:**

- Create: `migrations/0009_uploads_and_jobs.sql`
- Create: `crates/domain/src/imports/{upload,job}.rs`
- Create: `crates/application/src/imports/{mod,create_upload,receive_upload,job_queue}.rs`
- Create: `crates/application/src/ports/{upload_repository,job_repository}.rs`
- Create: `crates/postgres/src/{uploads,jobs}.rs`
- Create: `crates/postgres/tests/{upload_state_machine,job_leasing}.rs`
- Create: `crates/http/src/routes/uploads.rs`
- Create: `crates/http/tests/upload_routes.rs`
- Modify: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces `CreateUpload`, `ReceiveUpload`, and `GetUploadStatus` use cases.
- Produces `JobQueue::{enqueue,lease,heartbeat,succeed,retry,fail}` where `lease` uses `FOR UPDATE SKIP LOCKED`.
- Produces `POST /api/v1/libraries/{library_id}/uploads`, `PUT .../{upload_id}/content`, and `GET .../{upload_id}`.

- [ ] **Step 1: Write failing upload-state tests**

Assert only the documented transitions are legal: Created → Receiving → Received → Queued → Validating → Importing → Ready/Duplicate, with Failed/Expired/RetryWait paths. Assert declared size is required, 1 GiB maximum is enforced before body receipt, streaming stops if actual bytes exceed declared/reserved limits, retries reuse the upload ID safely, and reader cannot create or inspect uploads.

- [ ] **Step 2: Implement streaming receipt without whole-file buffering**

Read bounded chunks from Axum `Body`, append through `BlobStore`, update SHA-256 and byte count incrementally, and cap both per-chunk and total bytes. On disconnect or validation failure, persist a recoverable/failed state and release or resize the reservation exactly once. Do not hold a database transaction during network transfer.

- [ ] **Step 3: Create upload/job schema and lease semantics**

Create `upload_sessions`, `background_jobs`, and `job_attempts`. Store stable kind/state enums through check constraints, JSON payload only for versioned job input, idempotency key uniqueness, `lease_owner`, `lease_expires_at`, heartbeat, attempts, next-run time, safe error code/summary, and timestamps. Worker lease acquisition is one SQL statement with `SKIP LOCKED`; expired leases become eligible without a separate unlock operation.

- [ ] **Step 4: Expose authorized upload routes**

Require `holding.edit`, accept only `application/epub+zip` or `application/octet-stream` with an `.epub` filename in this slice, and return `202` plus a status resource. Do not reveal whether the hash exists in another library. Update OpenAPI with request size, states, problem codes, and retry semantics.

- [ ] **Step 5: Verify and commit**

Test abrupt body termination, oversized streams, concurrent job leases, expired lease recovery, retry scheduling, and process restart against real PostgreSQL/local storage. Run the workspace gate. Commit:

```bash
git add migrations/0009_uploads_and_jobs.sql crates/domain crates/application crates/postgres crates/http openapi
git commit -m "feat: add resumable upload workflow and durable jobs"
```

### Task 10: Parse EPUB Safely into a Neutral Publication Package

**Files:**

- Create: `crates/epub/Cargo.toml`
- Create: `crates/epub/src/{lib,error,archive,container,package,navigation,sanitize}.rs`
- Create: `crates/epub/tests/{valid_epub,malicious_epub,sanitize_content}.rs`
- Create: `tests/fixtures/epub/README.md`
- Create: `tests/fixtures/epub/generate-fixtures.rs`

**Interfaces:**

- Produces `EpubParser::inspect(&mut (impl Read + Seek), ParserLimits) -> Result<ParsedPublication, EpubError>`.
- Produces `ParsedPublication { metadata, spine, resources, toc, cover, warnings }` with normalized internal hrefs and no filesystem paths.
- Produces `ContentSanitizer::transform(html, ResourceResolver) -> SanitizedContent`.

- [ ] **Step 1: Generate licensed deterministic fixtures and failing tests**

Generate small EPUB 3 fixtures in test setup rather than committing copyrighted books: valid navigation/cover/CSS/image, malformed container, missing package, traversal path, absolute path, duplicate normalized path, external URL, script/events/forms/iframe/object/meta-refresh, excessive entry count, excessive expansion ratio, excessive nesting, and encrypted content. Assert stable error codes rather than ZIP-library strings.

- [ ] **Step 2: Implement bounded ZIP inspection**

Normalize EPUB paths once with a dedicated `EpubPath` value object. Reject absolute/backslash/traversal/NUL paths and duplicates after normalization. Enforce configurable entry count, total uncompressed bytes, compression ratio, path depth, individual resource size, and processing deadline before allocating from attacker-controlled sizes. Never call `ZipArchive::extract`.

- [ ] **Step 3: Parse container, package, metadata, spine, and navigation**

Use namespace-aware XML parsing. Preserve unknown metadata as warnings, not ad hoc columns. Require one readable package document and valid spine references. Convert authors/titles/languages/identifiers into neutral structs; do not create WEMI IDs or perform catalog merging in the parser.

- [ ] **Step 4: Sanitize requested HTML resources**

Remove executable/interactive elements and attributes, block external schemes and protocol-relative URLs, rewrite internal relative URLs through `ResourceResolver` to opaque resource IDs, and emit content compatible with `default-src 'none'`. CSS handling rejects `@import`, external `url()`, scripting URLs, and unsafe constructs while preserving safe EPUB layout rules. Add property tests ensuring transformed output contains no external URL or executable element.

- [ ] **Step 5: Verify and commit**

Run parser/sanitizer tests under normal test mode and with reduced parser limits. Confirm fixture generation is deterministic and source ZIP hash is unchanged. Run workspace gate. Commit:

```bash
git add crates/epub tests/fixtures/epub
git commit -m "feat: add bounded EPUB parsing and sanitization"
```

### Task 11: Persist WEMI Catalog, Holdings, Items, and Publication Packages

**Files:**

- Create: `migrations/0010_catalog.sql`
- Create: `crates/domain/src/catalog/{mod,wemi,holding,item,publication_package,content_unit}.rs`
- Create: `crates/application/src/catalog/{mod,import_publication}.rs`
- Create: `crates/application/src/ports/catalog_repository.rs`
- Create: `crates/postgres/src/catalog.rs`
- Create: `crates/application/tests/import_catalog.rs`
- Create: `crates/postgres/tests/{catalog_constraints,catalog_visibility}.rs`

**Interfaces:**

- Produces `ImportPublicationCatalog::execute(ImportCatalogCommand) -> ImportCatalogResult`; its catalog port finalizes catalog, quota, and success audit in one PostgreSQL transaction.
- Produces `ImportCatalogResult::{Created { item_id, package_id }, Duplicate { item_id }}`.
- Produces catalog queries that start from an authorized Holding/Item; no public “list all manifestations” repository method exists.

- [ ] **Step 1: Write failing catalog behavior tests**

Assert import creates Work → Expression ↔ Manifestation, Holding, Item, original ItemAsset, PublicationPackage/resources/TOC, and optional cover asset. Same library plus same original Blob returns the active existing Item. Different libraries get distinct Holding/Item and may share Blob. Similar metadata never auto-merges. A library has at most one active Holding per Manifestation; an Item belongs to one Holding.

- [ ] **Step 2: Implement explicit WEMI construction policy**

Treat each unmatched EPUB import as a new Work/Expression/Manifestation aggregate in this slice. Reuse a Manifestation only for exact previously parsed Blob/package identity or an explicit existing association; do not infer identity from title, author, identifier, or text. Keep parser metadata mapped through named constructors so external strings cannot directly create persistence rows.

- [ ] **Step 3: Create catalog/storage relation schema**

Create `works`, `expressions`, `manifestations`, `manifestation_expressions`, `holdings`, `items`, `item_assets`, `manifestation_assets`, `content_units`, `manifestation_units`, `publication_packages`, `publication_resources`, and `package_toc_entries`. Use explicit ordering keys, normalized href uniqueness within a package, `(blob_id, parser_profile_version)` package uniqueness, active Holding uniqueness, and foreign-key deletion rules that never cascade from one Item to shared Blob bytes. Map spine/navigation structure to ContentUnit and ManifestationUnit plus Locators without copying chapter HTML into PostgreSQL.

- [ ] **Step 4: Enforce visibility and duplicate races**

Enable forced RLS on Holding/Item and library-owned relations. Global WEMI/package/Blob tables receive no general API enumeration grant; authorized queries join from visible Holding. Serialize same-library finalization by a transaction-scoped advisory lock derived from library plus Blob identity, then recheck for an existing active original Item. Cross-library physical lookup runs only in the controlled import path and returns no timing-sensitive result to the caller.

- [ ] **Step 5: Verify and commit**

Run constraint, duplicate-race, cross-library isolation, and global-catalog enumeration tests against PostgreSQL. Assert deleting one Item does not delete shared Blob/package rows. Run workspace gate. Commit:

```bash
git add migrations/0010_catalog.sql crates/domain crates/application crates/postgres
git commit -m "feat: persist WEMI catalog and logical library items"
```

### Task 12: Orchestrate the Import Worker as an Idempotent Saga

**Files:**

- Create: `crates/application/src/imports/{process_import,cleanup}.rs`
- Create: `crates/application/src/ports/publication_parser.rs`
- Create: `crates/application/src/ports/{import_repository,import_cleanup_repository}.rs`
- Create: `crates/application/tests/{process_import,cleanup}.rs`
- Create: `apps/worker/src/{main,runner,handlers}.rs`
- Create: `apps/worker/tests/import_recovery.rs`
- Create: `crates/postgres/src/imports.rs`
- Modify: `crates/domain/src/imports/job.rs`
- Modify: `crates/application/src/ports/job_repository.rs`
- Modify: `crates/application/src/imports/job_queue.rs`
- Modify: `crates/postgres/src/jobs.rs`
- Modify: `crates/postgres/tests/{job_leasing,import_cleanup,upload_state_machine}.rs`
- Modify: `crates/epub/src/lib.rs`
- Modify: `crates/storage-local/src/lib.rs`
- Modify: `migrations/0009_uploads_and_jobs.sql`
- Modify: `migrations/0010_catalog.sql`

**Interfaces:**

- Produces `ProcessImportJob::execute(LeasedJob) -> Result<JobOutcome, JobFailure>`.
- Produces `JobFailure::{Permanent { code, summary }, Transient { code, retry_at }, OperatorRequired { code, summary }}`.
- Produces cleanup job kinds for expired uploads/reservations, failed-file purge, and later Blob GC.

- [ ] **Step 1: Write failing saga and crash-recovery tests**

For each boundary—received record, Blob promotion, parser completion, catalog transaction, quota consume, job success—inject one crash/failure, recreate the persisted Worker service, and assert one visible Item, one logical quota charge, no lost ready Blob, and an eventually terminal job. Include at least one real operating-system multi-process Worker race. Assert malformed EPUB fails permanently, transient I/O/database errors retry with jittered exponential backoff, and persistent configuration/schema/space errors become a distinct durable operator-required state.

- [ ] **Step 2: Implement a narrow Worker runner**

The runner leases one job, opens a tracing span, dispatches by a closed `JobKind`, heartbeats while bounded work runs, and maps typed failure to queue state. It does not know EPUB/catalog details. Concurrency is configurable and defaults to `max(1, available_parallelism / 2)`; a semaphore and queue polling backoff provide backpressure.

- [ ] **Step 3: Implement idempotent import orchestration**

Use the upload/job ID as the saga identity. Reconcile existing staging/location/catalog state before each action. Task 9 owns the physical, idempotent staging-to-Blob promotion boundary; this Worker must validate and reconcile that durable promotion outcome rather than repeat or bypass it. Parse or reuse `(blob, parser_profile_version)`, finalize catalog and consume quota in one database transaction, then clean staging. Never mark Ready before the catalog transaction commits. Duplicate consumes no new logical bytes and releases the reservation.

- [ ] **Step 4: Add bounded cleanup handlers**

Expire abandoned Created/Received uploads, release expired reservations, and purge quarantined failed bytes after 24 hours. Cleanup kinds are durable closed job kinds dispatched by the Worker. Each pass resumes a persisted cursor/time boundary, uses a limited batch and `SKIP LOCKED`, performs idempotent storage deletion, and advances the boundary only after the stable pass completes; it must not scan or lock the whole table.

- [ ] **Step 5: Verify and commit**

Run fault-injection tests, multiple concurrent Worker processes, a concurrency value of 1, and the workspace gate. Commit:

```bash
git add apps/worker crates/domain crates/application crates/epub crates/storage-local crates/postgres migrations
git commit -m "feat: process EPUB imports with an idempotent worker saga"
```

### Task 13: Expose Library Catalog and Item Detail APIs

**Files:**

- Create: `crates/application/src/catalog/{list_library_books,get_item}.rs`
- Create: `crates/http/src/routes/catalog.rs`
- Create: `crates/http/tests/catalog_routes.rs`
- Modify: `crates/postgres/src/catalog.rs`
- Modify: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces `ListLibraryBooks::execute(actor, library_id, PageRequest) -> Page<BookSummary>`.
- Produces `GetItem::execute(actor, library_id, item_id) -> ItemDetail`.
- Produces `GET /api/v1/libraries/{library_id}/books` and `GET /api/v1/libraries/{library_id}/items/{item_id}`.

- [ ] **Step 1: Write failing list/detail tests**

Assert “全部图书” lists only active accessible Holdings in the selected Library, uses stable keyset pagination, returns one row per Holding rather than per shared Blob, and includes explicit `can_read`/`can_download` capabilities computed by authorization. Assert unrelated users receive anti-enumerating 404 and cannot infer global catalog count.

- [ ] **Step 2: Implement read models separate from write aggregates**

Define small transport-neutral `BookSummary` and `ItemDetail` projections. Query from visible Holding through Manifestation/Expression/Work, never from global Work outward. Do not return internal parser profile, Blob hash, storage namespace/key, local path, audit IDs, or another library’s metadata.

- [ ] **Step 3: Implement thin routes and OpenAPI**

Validate cursor/limit, cap page size, invoke one use case, and map projections. Add Chinese/English display-ready field labels only in Web catalogs, not API values. Document capability booleans, pagination cursor opacity, ETags where applicable, and problem responses.

- [ ] **Step 4: Verify SQL and authorization behavior**

Test pagination stability under concurrent insert, query-count bounds to prevent N+1 behavior, owner/editor/reader equality for view permission, and reader download capability under both library settings.

- [ ] **Step 5: Commit**

Run focused tests and workspace gate. Commit:

```bash
git add crates/application crates/postgres crates/http openapi
git commit -m "feat: expose authorized library catalog queries"
```

### Task 14: Serve Readium-Style Manifests and Sanitized EPUB Resources

**Files:**

- Create: `crates/application/src/reader/{mod,get_manifest,get_resource}.rs`
- Create: `crates/application/src/ports/publication_resource_reader.rs`
- Create: `crates/http/src/routes/reader.rs`
- Create: `crates/http/tests/reader_routes.rs`
- Modify: `crates/postgres/src/catalog.rs`
- Modify: `crates/epub/src/lib.rs`
- Modify: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces `GetPublicationManifest::execute(actor, item_id) -> PublicationManifest`.
- Produces `GetPublicationResource::execute(actor, item_id, ResourceId) -> ResourceResponse`.
- Produces `GET /api/v1/items/{item_id}/manifest` and `GET /api/v1/items/{item_id}/resources/{resource_id}`.

- [ ] **Step 1: Write failing manifest/resource contract tests**

Assert the manifest contains metadata, reading order, resources, TOC, opaque resource IDs, media types, and self links without ZIP paths. Every resource request must independently authorize `item.read`. Assert revoked membership stops already-open readers on their next request. Assert external URLs, script, events, forms, nested frames, objects, and meta refresh cannot survive returned HTML/CSS.

- [ ] **Step 2: Build the Readium projection**

Map stored Package/resources/TOC into the documented Readium-style JSON without claiming unsupported features. Preserve stable resource IDs across requests for one Package. Include a package/version ETag and the exact Manifestation ID used by reading-state synchronization.

- [ ] **Step 3: Read one ZIP entry and transform it safely**

Resolve opaque ID to normalized entry only after authorization. Seek/read only that entry from the immutable EPUB, enforce decompressed-size limits again, sanitize/rewrite through the EPUB adapter, and return an allowed media type. Cache transformed bytes only in a bounded, disposable cache keyed by Blob/package/resource/sanitizer version; cache hits still require authorization.

- [ ] **Step 4: Apply browser isolation headers**

Return `Content-Security-Policy: default-src 'none'` with the minimum data/blob/style/font/img allowances actually required, `X-Content-Type-Options: nosniff`, restrictive referrer policy, and cache validators. The Web client must render documents in a sandboxed iframe without `allow-scripts`, `allow-forms`, or same-origin privilege unless a later security review proves a narrower safe mechanism.

- [ ] **Step 5: Verify and commit**

Run malicious EPUB tests through the HTTP boundary, membership revocation tests, cache authorization tests, Readium JSON snapshots, and workspace gate. Commit:

```bash
git add crates/application crates/postgres crates/epub crates/http openapi
git commit -m "feat: serve authorized EPUB reading resources"
```

### Task 15: Synchronize Reading Progress Across Devices

**Files:**

- Create: `migrations/0011_reading_state.sql`
- Create: `crates/domain/src/reader/{mod,locator,reading_state}.rs`
- Create: `crates/application/src/reader/{get_progress,update_progress}.rs`
- Create: `crates/application/src/ports/reading_repository.rs`
- Create: `crates/postgres/src/reader.rs`
- Create: `crates/application/tests/reading_progress.rs`
- Create: `crates/postgres/tests/reading_progress_concurrency.rs`
- Create: `crates/http/src/routes/progress.rs`
- Create: `crates/http/tests/progress_routes.rs`
- Modify: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces `ReadiumLocator { href, media_type, locations, text }` as validated domain data.
- Produces `GetReadingProgress` and `UpdateReadingProgress` using `device_id`, `client_mutation_id`, and `base_version`.
- Produces `GET/PUT /api/v1/manifestations/{manifestation_id}/progress`.

- [ ] **Step 1: Write failing state-machine tests**

Assert first write creates version 1; matching base version advances global/device state; repeating one mutation ID returns the original result; stale base version updates device state but not global state and returns both positions as a conflict; client timestamps do not order writes; “largest percentage wins” is never used. Assert progress is user-private and retained but unreadable as content after access loss.

- [ ] **Step 2: Validate transport-neutral Locators**

Accept Readium-compatible href/media type/locations/text context with bounded strings/arrays and no DOM node identity as the sole position. Keep JSON serialization at the adapter boundary; the domain uses typed fields plus an explicitly versioned extension map. Package and ContentUnit references are optional.

- [ ] **Step 3: Create schema and atomic update SQL**

Create `reading_states`, `device_reading_states`, and `reading_mutations`. Use unique `(user_id,manifestation_id)`, unique mutation ID per user, integer version checks, server timestamps, and forced user-level RLS. In one transaction insert mutation idempotency, update device state, compare/advance global version, and return either updated or conflict data.

- [ ] **Step 4: Expose versioned HTTP semantics**

Return an ETag derived from state version; accept `If-Match` and require the JSON `base_version` to agree. A conflict is RFC 9457 `409 progress_conflict` with safe extensions containing current global and device states. Authenticate through unified Actor, never directly through cookie parsing in the handler.

- [ ] **Step 5: Verify and commit**

Run concurrent updates from two devices, offline retry order permutations, RLS privacy tests, HTTP ETag tests, and workspace gate. Commit:

```bash
git add migrations/0011_reading_state.sql crates/domain crates/application crates/postgres crates/http openapi
git commit -m "feat: synchronize versioned reading progress"
```

### Task 16: Stream Permission-Controlled Original EPUB Downloads

**Files:**

- Create: `crates/application/src/catalog/download_item.rs`
- Create: `crates/http/src/routes/download.rs`
- Create: `crates/http/tests/download_routes.rs`
- Modify: `crates/application/src/authorization.rs`
- Modify: `crates/storage-local/src/lib.rs`
- Modify: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces `DownloadItem::authorize(actor, item_id) -> DownloadGrant` containing only opaque storage identity, size, media type, safe filename, and ETag material.
- Produces `GET/HEAD /api/v1/items/{item_id}/download` with single-range support.

- [ ] **Step 1: Write failing authorization and HTTP range tests**

Assert owner/editor can download, reader is denied by default, owner toggling the library setting grants reader download without changing read permission, and nonmember gets anti-enumerating 404. Cover full GET, HEAD, valid prefix/suffix/open ranges, unsatisfiable/multiple ranges, `If-None-Match`, interrupted client, and exact Content-Length/Content-Range.

- [ ] **Step 2: Separate read and download authorization**

Add no fallback from `item.download` to `item.read`. Resolve ItemAsset only after authorization and return a narrow grant; never expose path, storage key, namespace, or Blob hash to transport JSON/logs. Sanitize filenames to a quoted ASCII fallback plus RFC 5987 UTF-8 form and remove separators/control characters.

- [ ] **Step 3: Implement bounded streaming**

Parse at most one byte range, cap chunk buffers, seek through `BlobStore`, and stop promptly on client cancellation. Emit strong ETag derived from immutable Blob identity without publishing the raw hash, `Accept-Ranges: bytes`, safe `Content-Disposition`, EPUB media type, and `nosniff`.

- [ ] **Step 4: Audit downloads**

Record allowed download start with actor/library/item/request and byte range; record denied action with a reason code subject to anti-abuse aggregation. Never write content or filename to persistent audit metadata.

- [ ] **Step 5: Verify and commit**

Run route/range tests using a file larger than the response buffer, authorization matrix, cancellation test, and workspace gate. Commit:

```bash
git add crates/application crates/storage-local crates/http openapi
git commit -m "feat: stream authorized original EPUB downloads"
```

### Task 17: Deliver Verification, Invitation, and Reset Email Reliably

**Files:**

- Create: `migrations/0012_outbox.sql`
- Create: `crates/application/src/mail/{mod,enqueue,deliver}.rs`
- Create: `crates/application/src/ports/mail_repository.rs`
- Create: `crates/postgres/src/mail.rs`
- Create: `crates/application/tests/mail_delivery.rs`
- Modify: `apps/worker/src/handlers.rs`
- Create: `crates/http/src/routes/problems.rs`
- Modify: `deploy/example.folioharbor.toml`

**Interfaces:**

- Produces transactional `MailOutbox::enqueue(MailMessage)` and Worker `DeliverMailJob` through the existing `Mailer` port.
- Produces verification, invitation, and reset templates in plain text and HTML for `zh-CN` and `en`.
- Produces public problem documentation at `/problems/{code}`.

- [ ] **Step 1: Write failing outbox and content tests**

Assert a business transaction and its email intent commit or roll back together, retries use one idempotency key, SMTP transient/permanent failures classify correctly, plaintext and HTML variants contain the same single-use link, and logs never contain the full URL/token. Assert base URL host/scheme must match validated configuration.

- [ ] **Step 2: Create the outbox schema and transaction integration**

Create `mail_outbox` with recipient account ID plus delivery address, template code/version, locale, AEAD-encrypted one-time token payload plus key ID, idempotency key, state, attempts, next run, and timestamps. Use the application secret ring from Task 2 with a unique nonce and authenticated template/account context. Never persist or log plaintext token/link; decrypt only while rendering for one delivery attempt, zeroize the buffer, and erase ciphertext on terminal delivery or expiry.

- [ ] **Step 3: Implement SMTP delivery and localized templates**

Use TLS according to configuration, bounded timeouts, and no credential/debug body logging. Keep templates small and semantic; HTML is escaped and has no remote tracking resource. Existing reading remains healthy when SMTP is unavailable; readiness is false only when an enabled mail-required flow has no valid SMTP configuration, not during a transient outage.

- [ ] **Step 4: Serve stable problem documentation**

Render one public, locale-negotiated explanation per stable problem code without instance secrets. Cross-instance clients continue to use the JSON `code`; the type URI page is human-readable documentation.

- [ ] **Step 5: Verify and commit**

Run against a local SMTP capture service in integration tests, inspect captured text/HTML, force retry/failure, scan logs for test token values, then run workspace gate. Commit:

```bash
git add migrations/0012_outbox.sql crates/application crates/postgres crates/http apps/worker deploy
git commit -m "feat: deliver transactional account and invitation email"
```

### Task 18: Implement Deletion, Recovery, and Safe Blob Garbage Collection

**Files:**

- Create: `migrations/0013_deletion_and_gc.sql`
- Create: `crates/domain/src/catalog/lifecycle.rs`
- Create: `crates/application/src/catalog/{delete_item,restore_item,garbage_collect}.rs`
- Create: `crates/application/tests/item_lifecycle.rs`
- Create: `crates/postgres/src/garbage_collection.rs`
- Create: `crates/postgres/tests/blob_gc.rs`
- Modify: `apps/worker/src/handlers.rs`
- Modify: `crates/http/src/routes/catalog.rs`
- Modify: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces `DeleteItem`, `RestoreItem`, and Worker `CollectGarbage` use cases.
- Produces explicit `Active`, `Deleted`, `PurgeEligible`, and `Purged` lifecycle states with timestamps.

- [ ] **Step 1: Write failing lifecycle tests**

Assert delete revokes access immediately, restore succeeds within 7 days, purge eligibility begins after 7 days, and Blob deletion waits another 24 hours. Assert shared ItemAsset/ManifestationAsset references prevent collection. Assert Package/resources are removable cache-like derivatives, ReadingState retains Manifestation/ContentUnit/Locator while package ID becomes null, audit survives, and storage deletion failure remains retryable.

- [ ] **Step 2: Create explicit lifecycle schema**

Add state/deleted/purge timestamps and indexes used by bounded cleanup. Add nullable ReadingState package FK with `ON DELETE SET NULL`. Do not use cascading deletion from Library/Holding/Item into Blob or audit. Add constraints that permit only valid timestamp/state combinations.

- [ ] **Step 3: Implement delete/restore authorization**

Require `holding.edit`, lock Item, transition idempotently, update logical quota only at the approved lifecycle point, and write audit in the same transaction. Removing a member remains independent and immediately changes access without touching Item state.

- [ ] **Step 4: Implement two-phase GC**

Select a limited `SKIP LOCKED` batch, recheck authoritative references in the transaction, detach/delete rebuildable Package rows and mark BlobLocation purge-pending, commit, delete storage idempotently, then mark purged. A newly created reference must either lock/conflict with the candidate or make the final recheck fail. Never rely on a manually maintained authoritative reference counter.

- [ ] **Step 5: Verify and commit**

Run shared-Blob deletion, concurrent import-versus-GC, storage failure/retry, progress preservation, quota release, and audit retention tests. Run workspace gate. Commit:

```bash
git add migrations/0013_deletion_and_gc.sql crates/domain crates/application crates/postgres crates/http apps/worker openapi
git commit -m "feat: add recoverable item deletion and safe blob GC"
```

### Task 19: Establish the Strict TypeScript Web Shell and Authentication UX

**Files:**

- Create: `package.json`
- Create: `pnpm-workspace.yaml`
- Create: `web/package.json`
- Create: `web/{tsconfig.json,vite.config.ts,vitest.config.ts,eslint.config.js,index.html}`
- Create: `web/src/main.tsx`
- Create: `web/src/app/{App.tsx,router.tsx,providers.tsx,layout.tsx}`
- Create: `web/src/api/{client.ts,generated.ts,problem.ts}`
- Create: `web/src/i18n/{index.ts,zh-CN.json,en.json}`
- Create: `web/src/features/auth/{api.ts,session.ts,LoginPage.tsx,RegisterPage.tsx,VerifyEmailPage.tsx,ForgotPasswordPage.tsx,ResetPasswordPage.tsx}`
- Create: `web/src/test/{setup.ts,server.ts,render.tsx}`
- Create: `web/src/features/auth/auth.test.tsx`
- Modify: `openapi/folioharbor-v1.yaml`

**Interfaces:**

- Produces generated OpenAPI types in `generated.ts`; hand-written API functions may compose them but may not redefine server DTOs.
- Produces `apiClient.request` with cookie credentials, CSRF injection for unsafe calls, RFC 9457 decoding, request cancellation, and no automatic unsafe retry.
- Produces `useSession()` with `anonymous`, `loading`, and `authenticated` states.

- [ ] **Step 1: Scaffold strict Web tooling and write a failing auth-flow test**

Use React, React Router, TanStack Query, i18next, Testing Library, MSW, Vitest, ESLint, and `openapi-typescript`. Enable `strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, and `useUnknownInCatchVariables`; disallow explicit `any` and floating promises. The first test registers, sees verification-required feedback, verifies, logs in, and lands in the authenticated shell using only mocked HTTP contracts.

Run `pnpm --dir web test -- --run auth.test.tsx`. Expected: FAIL because the Web app does not exist.

- [ ] **Step 2: Generate types and implement one HTTP boundary**

Add `pnpm web:generate-api` and a CI check that generation leaves no diff. `client.ts` owns URL construction, JSON/problem parsing, CSRF header, cookie credentials, and abort signals. Feature code calls named endpoint functions and never calls `fetch` directly. Problem codes map to stable UI messages through i18n; unknown codes show request ID and a safe generic message.

- [ ] **Step 3: Implement accessible authentication pages**

Use real labels, field-level errors linked with `aria-describedby`, focus the error summary, announce async status, preserve keyboard order, and avoid revealing whether an email exists. Disable submit only while the same mutation is pending; cancellation/navigation must not leave stale success banners. Add both locale catalogs with identical keys and a locale switch that does not alter API codes.

- [ ] **Step 4: Implement the authenticated shell**

Keep current account/session query in one hook. Route guards render loading/anonymous/authenticated states explicitly without redirect loops. Add account session listing and revoke-one/revoke-all actions. Do not add library pages before Task 20.

- [ ] **Step 5: Verify and commit**

Run:

```bash
pnpm --dir web lint
pnpm --dir web typecheck
pnpm --dir web test -- --run
pnpm web:generate-api
git diff --exit-code web/src/api/generated.ts
```

Expected: all pass. Commit:

```bash
git add package.json pnpm-workspace.yaml pnpm-lock.yaml web openapi
git commit -m "feat: add accessible web authentication shell"
```

### Task 20: Build Library Switching, Catalog, Upload, and Collaboration UX

**Files:**

- Create: `web/src/features/libraries/{api.ts,queries.ts,LibrarySwitcher.tsx,LibraryLayout.tsx,SettingsPage.tsx}`
- Create: `web/src/features/catalog/{api.ts,queries.ts,BooksPage.tsx,ItemDetailPage.tsx,BookCard.tsx}`
- Create: `web/src/features/uploads/{api.ts,queries.ts,UploadPage.tsx,UploadStatus.tsx}`
- Create: `web/src/features/members/{api.ts,queries.ts,MembersPage.tsx,InvitationPage.tsx}`
- Create: `web/src/features/libraries/library-flow.test.tsx`
- Create: `web/src/features/uploads/upload-flow.test.tsx`
- Create: `web/src/features/members/member-flow.test.tsx`
- Modify: `web/src/app/router.tsx`
- Modify: `web/src/app/layout.tsx`
- Modify: `web/src/i18n/{zh-CN.json,en.json}`

**Interfaces:**

- Consumes generated APIs and `useSession()` from Task 19.
- Produces current-library route state from `/libraries/:libraryId/...`; it is not a global mutable singleton.
- Produces upload status rendering for Created, Receiving, Received, Queued, Validating, Importing, RetryWait, Ready, Duplicate, Failed, and Expired.

- [ ] **Step 1: Write failing two-library navigation tests**

Assert personal and shared libraries appear in one switcher, current library is always visible, “全部图书” changes with the route library, direct links survive reload, and there is no instance-global all-books route. Assert owner/editor/reader controls follow server capabilities; hiding a button is not treated as authorization.

- [ ] **Step 2: Implement catalog and item detail projections**

Use keyset pagination with accessible load-more behavior, empty/error/loading states, semantic headings, and server capability booleans. Display “作品”, “版本/格式”, and “书库副本”; never expose Work IDs, Blob, Package, or storage terminology as primary user concepts. Item detail presents online reading and download as separate actions and says “仅在线阅读” when appropriate.

- [ ] **Step 3: Write failing upload lifecycle tests and implement streaming upload UX**

Test owner/editor upload and reader denial, client-side 1 GiB precheck, progress based on transmitted bytes, transition from transfer to background processing, reload-safe polling, Duplicate linking to existing Item, retryable/permanent failure copy, and cancellation. Use XHR only inside the upload API adapter if fetch upload progress is unavailable; components never own transport code.

- [ ] **Step 4: Implement members, invitations, and settings**

Owner can invite by email/role, change role, remove a member, and toggle reader download; editor/reader cannot. Prevent final-owner actions in UI based on server response, not duplicated owner-count assumptions. Invitation acceptance handles logged-out, wrong-account, unverified, expired, consumed, and successful states while retaining the personal library.

- [ ] **Step 5: Verify and commit**

Run Web lint/typecheck/all component tests, keyboard-only Testing Library interactions, and an automated accessibility scan for these pages. Commit:

```bash
git add web/src
git commit -m "feat: add web library collaboration and upload flows"
```

### Task 21: Build the EPUB Reader and Cross-Device Progress UX

**Files:**

- Create: `web/src/features/reader/{api.ts,locator.ts,ReaderPage.tsx,ReaderFrame.tsx,TableOfContents.tsx,ReadingSettings.tsx,ProgressSync.ts}`
- Create: `web/src/features/reader/{reader.test.tsx,progress-sync.test.ts}`
- Modify: `web/src/app/router.tsx`
- Modify: `web/src/i18n/{zh-CN.json,en.json}`

**Interfaces:**

- Consumes Readium-style manifest/resource and progress endpoints.
- Produces `ProgressSync` as a UI-independent state machine taking a `ProgressApi`, device ID, mutation-ID source, and clock; Android can implement the same server protocol later.
- Produces a stable per-install `device_id` that is resettable and not used as authentication.

- [ ] **Step 1: Write failing reader security and navigation tests**

Assert the iframe has a sandbox without script/forms/popups/top-navigation privileges, resources come only from authorized API URLs, TOC navigation uses opaque links, and no EPUB HTML is inserted into the parent DOM. Test keyboard TOC, focus return, reduced motion, font scaling, scroll/pagination preference, loading/error/access-revoked states, and locale-independent Locator data.

- [ ] **Step 2: Implement the isolated reader**

Fetch manifest through the typed API, fetch sanitized content as a Blob, and load it into the sandboxed frame without granting same-origin scripting. Keep pagination/continuous scroll as a user preference while reporting a Readium Locator, not a raw DOM node. Revoke object URLs after navigation/unmount.

- [ ] **Step 3: Write failing progress synchronization tests**

Use two fake devices. Cover debounce, unload/visibility flush using a bounded request, safe mutation retry, accepted version advance, stale-version conflict, display of global/device choices, offline queue ordering, and permission loss. No test may resolve conflicts by maximum percentage.

- [ ] **Step 4: Implement `ProgressSync` separately from React**

Use explicit states `idle`, `dirty`, `saving`, `synced`, `offline`, `conflict`, and `inaccessible`. Persist only pending mutations/device metadata needed for safe retry, with bounded storage. React components subscribe to state and ask the user to choose on conflict; the state machine calls API methods and contains no DOM code.

- [ ] **Step 5: Verify and commit**

Run reader/component/state-machine tests, Web lint/typecheck, an accessibility scan, and a browser test with two isolated contexts sharing one account. Commit:

```bash
git add web/src
git commit -m "feat: add secure EPUB reader and progress sync"
```

### Task 22: Add Operations, Observability, Bootstrap CLI, and Compose Deployment

**Files:**

- Create: `migrations/0014_operations.sql`
- Create: `crates/application/src/operations/{mod,health,bootstrap_admin,consistency_check}.rs`
- Create: `crates/postgres/src/operations.rs`
- Create: `crates/http/src/routes/health.rs`
- Create: `crates/http/src/middleware/telemetry.rs`
- Modify: `apps/{api,worker,cli}/src/main.rs`
- Create: `apps/cli/src/{commands,admin,migrate,check_storage}.rs`
- Create: `deploy/{compose.yaml,example.env,README.md}`
- Create: `deploy/postgres/init/README.md`
- Create: `docs/operations/{configuration,backup-and-restore,incident-response}.md`
- Create: `crates/http/tests/health_routes.rs`
- Create: `apps/cli/tests/admin_bootstrap.rs`

**Interfaces:**

- Produces `/health/live` and `/health/ready` with no sensitive dependency detail.
- Produces `folioharbor admin create --email admin@example.com`, password read from TTY, and `folioharbor storage check`.
- Produces one official Compose topology: migration init, PostgreSQL 18, API, Worker, shared Blob/staging volume, and development SMTP capture profile.

- [ ] **Step 1: Write failing bootstrap and health tests**

Assert liveness depends only on process loop; readiness checks database, exact compatible schema, storage capability/free reserve, required configuration, and presence of a system administrator. Before bootstrap readiness returns a safe `bootstrap_required` state. First public registration never creates a system administrator. System administrator has no implicit content read permission.

Until at least one system administrator exists, the API may expose health/problem endpoints but must reject public registration with `503 bootstrap_required`; bootstrap does not depend on public registration. Add the transition test that registration becomes available only after successful CLI bootstrap when public registration is enabled.

- [ ] **Step 2: Add operations schema and CLI bootstrap**

Create `system_administrators` keyed to user account and separate from library roles. CLI connects with owner/bootstrap credentials, securely prompts twice on TTY, creates a verified account plus system-admin row transactionally, and refuses password arguments/environment values. Re-running for the same email reports an idempotent safe outcome.

- [ ] **Step 3: Add structured telemetry**

Initialize JSON tracing with request/job/trace IDs and W3C `traceparent`, OpenTelemetry API with configurable exporter, and metrics for latency, errors, queue depth, retries, upload bytes, free storage, and pool state. Reject high-cardinality labels: no email, title, user ID, Item ID, Blob hash, or path. Redact headers/cookies/tokens and attach internal errors by error ID only.

- [ ] **Step 4: Build and test official Compose deployment**

Migration completes before API/Worker; runtime processes use distinct role secrets and never owner credentials. Mount Blob/staging in one shared volume with documented permissions. Add healthchecks, graceful shutdown, resource-limit examples, Worker concurrency 1 example, and HTTPS reverse-proxy guidance without bundling a mandatory proxy. Secrets support environment or `_FILE`; committed examples contain no real secrets.

- [ ] **Step 5: Document backup/restore and consistency checking**

Document PostgreSQL plus Blob volume as one business backup set, schema version and Blob watermark recording, restore ordering, and post-restore `storage check` for missing Blob, orphan location, and hash mismatch. Do not claim crash-consistent cross-volume snapshots unless the operator provides them. Run CLI/health/Compose config tests and workspace/Web gates. Commit:

```bash
git add migrations/0014_operations.sql crates apps deploy docs/operations
git commit -m "feat: add deployment operations and observability"
```

### Task 23: Prove the Complete Vertical Slice and Lock the Release Gates

**Files:**

- Create: `web/playwright.config.ts`
- Create: `web/e2e/{auth-library.spec.ts,upload-read.spec.ts,permissions.spec.ts,progress.spec.ts,recovery.spec.ts}`
- Create: `tests/e2e/{README.md,compose.test.yaml}`
- Create: `tests/security/README.md`
- Create: `scripts/check-migrations.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Modify: `docs/README.md`
- Create: `docs/operations/release-checklist.md`

**Interfaces:**

- Consumes all preceding production interfaces; produces no new business abstraction.
- Produces CI jobs `rust`, `web`, `postgres`, `e2e`, `supply-chain`, and `container-smoke` with explicit dependencies and uploaded diagnostics free of secrets/content.

- [ ] **Step 1: Write the failing two-user E2E journey**

Against a clean Compose database: bootstrap admin; Alice registers/verifies and gets a personal library; Alice invites Bob as reader; Bob registers/verifies/accepts and retains his personal library; Alice uploads generated EPUB; Worker reaches Ready; Bob reads; device A saves progress and device B observes it; Bob download is denied; Alice enables reader download; Bob completes Range download and verifies the original hash.

- [ ] **Step 2: Add adversarial authorization and EPUB journeys**

Prove editor can upload but not manage members, reader cannot upload, unrelated user receives anti-enumerating responses, wrong-library IDs cannot read jobs/catalog/resources/downloads, malicious EPUBs fail safely, resource revocation takes effect on the next request, and no response/log/artifact contains local path, storage key, plaintext token, Cookie, or cross-library hash information.

- [ ] **Step 3: Add resilience and concurrency journeys**

Kill API during upload, kill Worker after Blob promotion and during catalog finalization, restart both, run simultaneous quota reservations, upload the same EPUB concurrently in one and two libraries, conflict two progress writers, delete one shared Item, force one GC storage failure, and verify the accepted terminal state/invariants after recovery.

- [ ] **Step 4: Complete migration and release CI**

CI creates PostgreSQL 18 from zero, runs every migration, verifies minimal API/Worker read/write plus RLS, reruns migrate idempotently, and checks schema metadata. Because this is the first release, do not fabricate a prior-version fixture; after the first tag, commit the supported previous schema fixture and enable upgrade testing before the next release. Add formatting, Clippy, Rust/Web tests, OpenAPI generation diff, Playwright, dependency vulnerability/license/secret scans, image builds, and minimal Compose startup.

- [ ] **Step 5: Run the complete verification matrix**

Run fresh, without relying on earlier task output:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
pnpm --dir web lint
pnpm --dir web typecheck
pnpm --dir web test -- --run
pnpm --dir web exec playwright test
docker compose -f deploy/compose.yaml config --quiet
scripts/check-migrations.sh
```

Expected: every command exits 0 and Playwright reports all scenarios passed. Review the diff for Clean Code: domain names, single responsibilities, dependency direction, explicit errors, focused files, no suppressed lint without reasons, no stale code, and no scope-excluded feature.

- [ ] **Step 6: Commit the release gate**

```bash
git add .github README.md docs scripts tests web/e2e web/playwright.config.ts
git commit -m "test: verify first EPUB vertical slice end to end"
```

## Specification Coverage Matrix

| Specification section | Implemented and verified by |
| --- | --- |
| 1–4 document scope, goals, completion | Global Constraints; Tasks 23 |
| 5–7 topology, module boundaries, PostgreSQL | Tasks 1–3, 22 |
| 8 identity and library model | Tasks 4–7 |
| 9 WEMI, Holding, Item, Blob, dedup scope | Tasks 8, 11–12, 18 |
| 10 PublicationPackage and EPUB metadata | Tasks 10–12 |
| 11 reading state and devices | Tasks 15, 21 |
| 12 local identity and session security | Tasks 4–5, 17 |
| 13 authorization and RLS | Tasks 6–7, 23 |
| 14 upload/import state machine | Tasks 8–12 |
| 15 reader resources and isolation | Tasks 10, 14, 21 |
| 16 original-file download | Task 16 |
| 17 cross-client synchronization | Tasks 15, 19, 21 |
| 18 RFC 9457 error contract | Tasks 2, 5, 17 |
| 19 background failure/recovery | Tasks 9, 12, 23 |
| 20 deletion and Blob GC | Task 18 |
| 21 quota and physical capacity | Tasks 8–9 |
| 22 audit | Tasks 7, 16, 18 |
| 23 observability | Task 22 |
| 24 deployment and configuration | Tasks 2, 22 |
| 25 administrator bootstrap and email | Tasks 17, 22 |
| 26 backup boundary | Task 22 |
| 27–28 Web information architecture and UX | Tasks 19–21 |
| 29 shared API/future clients | Tasks 5, 13–16, 19, 21 |
| 30 testing strategy | Every task; Task 23 integration |
| 31 migrations | Tasks 3–9, 11, 15, 17–18, 22–23 |
| 32 CI gates and resource behavior | Tasks 1, 8–10, 12, 22–23 |
| 33 future compatibility without speculative implementation | Global Constraints and per-task interfaces |
| 34 acceptance scenarios | Task 23 |
| 35 standards | Tasks 2, 5, 10, 14–15, 19–23 |
| 36 implementation gate | Approved specification plus this plan |

## Final Definition of Done

- All 23 task commits exist in order or have equivalently scoped reviewed commits with traceable reasons.
- Every “must implement” item and completion criterion in the approved specification is covered by a passing automated test or an explicit operator verification in the release checklist.
- API, Worker, CLI, Web, and migrations start from the official Compose deployment on an empty machine with only documented prerequisites.
- Database roles and RLS tests prove runtime processes cannot bypass tenant isolation.
- The two-user EPUB journey, cross-device progress, independent download permission, failure recovery, and Blob lifecycle pass from a clean database.
- Documentation accurately states what is implemented and keeps all excluded future capabilities labeled as future work.
- The implementation has passed requirements review, code-quality review, security review, and `superpowers:verification-before-completion` with fresh evidence.

## New-Session Execution Entry

Start a new session in the FolioHarbor directory and say:

```text
使用 Superpowers 按 docs/superpowers/plans/2026-08-05-first-epub-vertical-slice.md 执行实施计划。
从 Task 1 开始，采用 subagent-driven-development；每个任务先做规格符合性审查，再做代码质量审查。
严格遵守 TDD、Clean Code 和每任务独立提交，不要提前实现后续任务。
```

If executing without subagents, replace `subagent-driven-development` with `executing-plans` and retain the same task/review/commit boundaries.
