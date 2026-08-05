# Task 9 implementation report

## Scope delivered

- Added migration 0009 with durable `upload_sessions`, `background_jobs`, and `job_attempts`, forced RLS, stable state/kind constraints, versioned JSON job input, idempotency keys, lease ownership/expiry, attempts, scheduling, and safe errors.
- Added atomic authorized upload creation/reservation plus separate API- and worker-only transition functions. The API surface can only advance receipt states; import lifecycle transitions require the worker role. Receipt resize/release is lock-protected and exactly once; catalog quota finalization remains reserved for Task 12.
- Added domain upload and job models, application upload/status/receipt/job-queue ports and use cases, PostgreSQL adapters, Axum upload routes, and OpenAPI contracts.
- Upload receipt streams bounded chunks to `BlobStore`, splits oversized body frames to at most 1 MiB, hashes incrementally, never buffers a whole EPUB, never holds a database transaction across network input, rejects declared sizes over 1 GiB before receipt, and makes interrupted/oversized receipts retryable with the same upload ID.
- Job leasing is a single `FOR UPDATE SKIP LOCKED` CTE. It atomically records expired attempts and creates the next attempt, with heartbeat, success, retry, and terminal failure operations.

## TDD evidence

RED was established before production implementation:

- `cargo test -p folioharbor-domain --test upload_workflow` failed because `imports::upload` did not exist (`task-9-red-domain.log`).
- `cargo test -p folioharbor-postgres --test upload_state_machine --test job_leasing` failed to compile because the upload/job repositories, domain types, and PostgreSQL adapters did not exist.
- `cargo test -p folioharbor-http --test upload_routes` initially returned 202 for a declared body above 1 GiB instead of the required 413, proving the pre-body limit assertion; the handler gate made it green.

Focused GREEN coverage:

- Domain transition graph, including Failed, Expired, and RetryWait branches.
- Interrupted and oversized streams clean staging and persist exactly one recoverable failure; large frames are split into `[1 MiB, remainder]` appends.
- Reader create/inspect denial, atomic reservation creation, failed receipt release exactly once, and safe retry with the same upload ID.
- API `Queued -> Validating` denial and worker-only lifecycle progression; Ready and Duplicate preserve the reservation for Task 12.
- Concurrent lease exclusion, expired-lease attempt recovery, heartbeat, retry timing, repository restart, and success.
- HTTP 202/Location/status resources, media and filename validation, 1 GiB rejection before body/service invocation, and OpenAPI limits/states/retry/problem contracts.

## Verification

All commands passed after the final changes:

- `cargo fmt --all --check`
- `SQLX_OFFLINE=true cargo check --workspace --all-targets --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo sqlx prepare --workspace -- --all-targets --all-features`
- `cargo sqlx prepare --workspace --check -- --all-targets --all-features`
- `FOLIOHARBOR_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:32771/postgres cargo test --workspace --all-features` against PostgreSQL 18.4
- `cargo deny check` (exit 0; repository-wide unmatched-license and duplicate-version warnings remain non-failing)
- `git diff --check`

All new fixed SQL in the Task 9 PostgreSQL adapters uses SQLx compile-time checked macros and has checked-in offline metadata.

## Review notes

- No later import-validation or parsing task was implemented early; Task 9 stops at the durable queue boundary.
- The HTTP/application layers do not expose cross-library hash existence.
- A promoted blob can outlive an interrupted database update; deterministic keys and resumable Receiving state now recover it with the same upload ID, while the planned garbage-collection task still owns abandoned-object cleanup.

## Specification review fix round 1

Focused RED evidence captured before the fixes:

- `task-9-fix-red-production-composition.log`: the API package had no production upload composition builder, and `main` retained `UnavailableUploadApi`.
- `task-9-fix-red-recovery.log`: failures after promotion, after Received persistence, and after enqueue all left states that rejected a same-ID retry with `upload_state_conflict`.
- `task-9-fix-red-process-restart.log`: a real PostgreSQL 18 + `LocalBlobStore` restart failed after a promoted receipt and injected finalize cut.
- `task-9-fix-red-expiry.log`: PostgreSQL reported that `upload_expire_worker(timestamptz,bigint)` did not exist.
- `task-9-fix-red-stale-leases.log`: stale `succeed` returned true after lease expiry.
- `task-9-fix-red-quota-boundary.log`: Ready prematurely changed quota to `(used=42,reserved=0,state=consumed)`.
- `task-9-fix-red-finalize-rollback.log`: a mismatched legacy Received recovery inserted one job despite returning false.

Fixes and GREEN proof:

- The production API now composes `UploadService` from the API-role `PgUploadRepository`, `PgAuthorizationRepository`, validated storage configuration, and `LocalBlobStore`; no worker pool is required. The real composition test creates and receives an EPUB to Queued.
- Receipt persistence, versioned validation-job insertion, reservation resize, and Queued transition are one API-only `SECURITY DEFINER` operation. The API role has no general job-table mutation path, and the generic receipt transition cannot queue work.
- Finalize is idempotent and validates all legacy-state fields before mutation. Repeated calls retain one job and one resized reservation; false outcomes leave no job behind.
- Receiving retries restage safely. Deterministic promotion reconciles an already-promoted object, and the real restart test reaches Queued with the original upload ID.
- Worker expiry moves only expired Created sessions to Expired and releases each reservation once.
- Heartbeat, succeed, retry, and fail all require a currently unexpired owned lease.
- Ready and Duplicate leave quota usage and the active reservation unchanged for the later import saga.

The existing Task 8 `LocalBlobStore` owns staging and final objects beneath one secure capability root. `storage.staging_root` therefore remains validated configuration but is not independently selectable by this adapter. This is non-blocking for Task 9 and is handed off as storage-adapter architecture debt rather than expanding Task 8 scope here.

## Code-quality review fix round 2

Focused RED evidence captured before production changes:

- `task-9-fix2-red-concurrent-put.log`: a second `Receiving -> Receiving` transition succeeded and replaced the active staging key.
- `task-9-fix2-red-receiving-expiry.log`: expiry processed one Created receipt but left an abandoned Receiving receipt and reservation live.
- `task-9-fix2-red-poison-job.log`: malformed job input was discovered only after the lease transaction committed, so the poison job remained leased instead of being quarantined.
- `task-9-fix2-red-dedup-scope.log`: `UploadService` had no configured dedup scope and always promoted into the instance namespace.

Fixes and GREEN proof:

- Receiving attempts now have a database-owned token and five-minute lease. A staging key is the external attempt capability; concurrent PUTs and stale abort/finalize calls cannot replace or mutate a newer attempt. Every bounded 1 MiB append renews the lease.
- The final candidate key and whether it is exclusively owned are persisted before promotion. Promotion is followed by a durable Received checkpoint, so a process restart finalizes the same upload without receiving bytes again.
- Expiry atomically fails stale Receiving attempts, releases quota once, and inserts an `upload_cleanups` record. Cleanup leasing uses `FOR UPDATE SKIP LOCKED`, has owner/expiry-bound acknowledgement, always deletes staging, and deletes a final object only for the upload-scoped Disabled dedup mode. Instance/Library content-addressed candidates are recorded as reconciled without deleting shared objects. Retry stays blocked until cleanup completes.
- Job payloads are constrained to exact version 1 plus a UUID upload ID. Leasing parses the complete batch before commit; malformed rows roll the batch back and are quarantined as `invalid_job_input`, allowing valid jobs to be leased on the next call.
- The validated application setting now selects Instance, Library, or Disabled domain dedup namespaces in production composition.

Final fix-round verification:

- `cargo fmt --all -- --check`
- `FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:32771/postgres cargo test --workspace`
- `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings`
- Fresh database `folioharbor_task9_fix2_sqlx` migrated from zero through migration 9.
- `DATABASE_URL=postgres://folioharbor_owner@127.0.0.1:32771/folioharbor_task9_fix2_sqlx cargo sqlx prepare --workspace -- --all-targets`
- The changed fixed-shape finalize statement remains `query_scalar!`; its checked-in metadata was replaced with the new eight-parameter signature.
- `git diff --check`

## Code-quality review fix round 3

Focused RED evidence captured before production changes:

- `/tmp/task-9-fix3-red-idle-heartbeat.log`: a paused body could not configure a periodic receipt heartbeat, so a slow or idle client could lose its database lease before another body frame arrived.
- `/tmp/task-9-fix3-red-promotion-disposition.log`: promotion returned only a storage key and could not distinguish a newly installed shared blob from a reused blob.
- `/tmp/task-9-fix3-red-cleanup-security.log`: recovery had no transaction-held cleanup guard, allowing a cleanup lease to expire while filesystem deletion was still running.
- The adversarial PostgreSQL test was added before its supporting production changes and exercised forged staging keys, mismatched final identities, and direct ownership escalation.

Fixes and GREEN proof:

- Receipt heartbeat is driven by an independent Tokio interval while awaiting the next body frame. A deterministic paused-time test proves an idle body is renewed, and a lost renewal stops receipt without deleting storage owned by a newer attempt.
- Blob promotion now reports `Installed` or `Reused`. Disabled final objects are owned only when installed by that upload. Newly installed Instance/Library blobs produce durable `installed_shared` reachability candidates; reused shared blobs never acquire ownership or a candidate.
- Cleanup ownership is fenced by a PostgreSQL row lock held across external deletion. Cancellation detaches the deletion task while its guard remains live. A deterministic independent-worker test proves that advancing beyond the former lease expiry cannot re-claim the row; retry becomes possible only after the guard completes, preventing stale cleanup from racing a newly promoted final.
- `SECURITY DEFINER` promotion functions validate exact staging capabilities, derive final namespace and identity from the stored dedup scope plus digest and byte count, and derive final ownership from the recorded promotion disposition. The API role cannot directly set ownership.

Final fix-round verification:

- `FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:32771/postgres cargo test --workspace`
- `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings`
- Fresh database `folioharbor_task9_fix3_sqlx` migrated from zero through migration 9.
- `DATABASE_URL=postgres://folioharbor_owner@127.0.0.1:32771/folioharbor_task9_fix3_sqlx cargo sqlx prepare --workspace -- --all-targets`
- `DATABASE_URL=postgres://folioharbor_owner@127.0.0.1:32771/folioharbor_task9_fix3_sqlx cargo sqlx prepare --check --workspace -- --all-targets`
- The changed fixed-shape upload-creation statement remains a `query_scalar!` macro with regenerated checked-in offline metadata.
- `cargo fmt --all --check`
- `git diff --check`
