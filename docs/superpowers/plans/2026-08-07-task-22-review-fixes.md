# Task 22 Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close every P1/P2 from the Task 22 independent review with real storage, PostgreSQL, OpenTelemetry, and composition regressions.

**Architecture:** Keep one capability root containing `objects/` and `staging/`, and make the storage port expose a non-destructive write probe plus a no-symlink inventory. Use the OpenTelemetry SDK span context as the only trace source, persist the originating W3C context on import jobs, and parent Worker attempt spans to that context. Replace startup/resource snapshots and leased-batch “depth” with live/periodic resource observers and a repository-defined backlog observation.

**Tech Stack:** Rust, Axum, tracing-opentelemetry/OpenTelemetry 0.30, SQLx/PostgreSQL 18, cap-std, Docker Compose.

## Global Constraints

- Preserve aggregate-only health/check output and the metric-label allowlist.
- Preserve distinct owner/API/Worker credentials and all existing RLS/grants.
- Every production behavior change begins with a test observed failing for that behavior.
- Staging and object data remain on one filesystem for atomic hard-link promotion.
- No credentials, paths, Blob identities, user identities, headers, cookies, or tokens enter logs/metrics.

---

### Task 1: Capability-Proven Readiness and Complete Storage Inventory

**Files:**
- Modify: `crates/application/src/ports/blob_store.rs`
- Modify: `crates/application/src/operations/{health,consistency_check}.rs`
- Modify: `crates/storage-local/src/{lib,operations,secure_fs}.rs`
- Test: `crates/application/tests/operations.rs`
- Test: `crates/storage-local/tests/storage_contract.rs`

**Interfaces:**
- Add `BlobStore::probe_write()` and `BlobStore::inventory()`.
- Add `BlobStoreInventory { keys: Vec<StorageKey>, invalid_locations: u64 }`.
- `LocalBlobStore::probe_write()` creates, syncs, removes, and directory-syncs a private unique file beneath `staging/.health/` through `SecureRoot`.

- [x] Add failing health tests where capacity succeeds but `probe_write` fails, and a Unix wrong-mode/read-only local-store regression.
- [x] Add failing consistency tests with an extra canonical object and invalid/symlink object entry.
- [x] Implement the bounded write probe and secure fixed-depth object inventory without following symlinks.
- [x] Compare database and filesystem key sets in both directions and rerun both suites green.

### Task 2: Concurrent Bootstrap Idempotence

**Files:**
- Modify: `migrations/0027_operations.sql`
- Test: `crates/postgres/tests/operations.rs`

**Interfaces:**
- `operations_bootstrap_admin` serializes on a stable normalized-email advisory transaction lock before reading/inserting.

- [x] Add a two-connection PostgreSQL 18 test that holds transaction A open while transaction B bootstraps the same email.
- [x] Observe B fail with the current unique-key race after A commits.
- [x] Add `pg_advisory_xact_lock(hashtextextended(p_normalized_email, 0))` before the lookup.
- [x] Verify A returns `created`, B returns `already_administrator`, and only one account/admin row exists.

### Task 3: SDK Trace Identity and API-to-Worker Propagation

**Files:**
- Modify: `crates/http/src/middleware/telemetry.rs`
- Modify: `crates/http/src/routes/uploads.rs`
- Modify: `crates/application/src/imports/{create_upload,receive_upload}.rs`
- Modify: `crates/application/src/ports/upload_repository.rs`
- Modify: `crates/domain/src/imports/job.rs`
- Modify: `crates/postgres/src/{uploads,jobs}.rs`
- Modify: `migrations/0027_operations.sql`
- Modify: `apps/worker/src/runner.rs`
- Test: `crates/http/tests/telemetry.rs`
- Test: `crates/postgres/tests/job_leasing.rs`
- Test: `apps/worker/src/runner.rs`

**Interfaces:**
- `RequestTraceContext` contains the SDK-injected W3C `traceparent` and real SDK trace ID.
- `ReceiveUploadRequest`/`FinalizeUploadReceipt` carry `Option<String>` traceparent.
- `background_jobs` stores bounded `origin_request_id` and validated `origin_traceparent`; `LeasedJob` exposes them.
- Worker spans call `set_parent` with the extracted origin and record the SDK context trace ID.

- [x] Add an in-memory exporter HTTP test proving response/extension trace IDs equal the exported server span and inbound W3C parents are honored.
- [x] Add a real enqueue/lease test proving request ID and traceparent survive API finalization into `LeasedJob`.
- [x] Add a Worker in-memory exporter test proving the attempt span shares the originating trace ID.
- [x] Replace UUID trace generation/echo with SDK extraction, span context injection, persistence, and Worker parenting; rerun the three regressions green.

### Task 4: Real Backlog Metrics

**Files:**
- Modify: `crates/application/src/ports/job_repository.rs`
- Modify: `crates/application/src/imports/job_queue.rs`
- Modify: `crates/postgres/src/jobs.rs`
- Modify: `apps/worker/src/runner.rs`
- Test: `crates/postgres/tests/job_leasing.rs`
- Test: `apps/worker/src/runner.rs`

**Interfaces:**
- Add `JobBacklog { runnable: u64, scheduled_retry: u64 }` and `JobRepository::backlog(now)`.
- `runnable` counts due pending/retry rows plus expired leases; `scheduled_retry` counts future `retry_wait` rows.

- [x] Add a PostgreSQL regression with backlog greater than concurrency and retry rows on both sides of `next_run_at`.
- [x] Observe the missing backlog API failure.
- [x] Implement one bounded aggregate query and emit `state=runnable` / `state=scheduled_retry` gauges before leasing.
- [x] Remove leased-batch-as-depth and rerun repository/runner tests green.

### Task 5: Live Resource Gauges

**Files:**
- Modify: `crates/http/src/middleware/telemetry.rs`
- Modify: `apps/{api,worker}/src/main.rs`
- Test: `crates/http/tests/telemetry.rs`

**Interfaces:**
- Add a periodic sampler backed by current pool `size`/`num_idle` values and `Arc<dyn BlobStore>`; failed storage samples omit observations instead of publishing stale success.

- [x] Add a sampler test that mutates pool/storage observations and samples twice.
- [x] Observe the current startup-only API cannot reflect the second value.
- [x] Refresh pool open/idle gauges and free-storage sampling every 15 seconds with bounded state labels.
- [x] Retain the API sampler task and Worker interval for process lifetime and rerun metric tests green.

### Task 6: One Truthful Storage Root in Compose and Documentation

**Files:**
- Modify: `crates/application/src/config/{raw,types}.rs`
- Modify: `crates/application/src/config.rs`
- Modify: `crates/application/tests/config_contract.rs`
- Modify: `apps/api/tests/upload_composition.rs`
- Modify: `deploy/{compose.yaml,README.md}`
- Modify: `docs/operations/{configuration,backup-and-restore,incident-response}.md`

**Interfaces:**
- `storage.root` is the only configured capability root; objects live at `<root>/objects`, transient staging at `<root>/staging`.
- `FOLIOHARBOR_STORAGE_STAGING_ROOT` becomes an unknown setting instead of a dead accepted value.

- [x] Add a failing TOML config regression that rejects the removed staging-root key.
- [x] Update composition to use and verify the single configured storage root.
- [x] Remove `staging_root`, set Compose root to `/var/lib/folioharbor`, initialize that root privately, and update operator prose.
- [x] Validate rendered Compose topology and rerun config/composition tests green.

### Task 7: Full Verification, Report, and Corrective Commit

**Files:**
- Modify: `.superpowers/sdd/2026-08-05-first-epub-vertical-slice/task-22-report.md`

- [x] Run focused RED/GREEN suites, full PostgreSQL 18 workspace tests, fmt/check/scoped Clippy, Compose config, shell syntax, and `git diff --check`.
- [x] Re-read every P1/P2 and map it to code plus a regression in the report.
- [x] Remove only the disposable review-fix PostgreSQL container.
- [x] Commit source/tests/docs/config/report-eligible changes with a Task 22 review-fix message; do not push.

## Self-review

- Spec coverage: all three P1 and four P2 findings map to a task and an observable regression.
- Placeholder scan: no deferred implementation or unspecified behavior remains.
- Type consistency: storage inventory, request trace context, durable job origin, backlog, and observer guards each have one named producer/consumer path.
