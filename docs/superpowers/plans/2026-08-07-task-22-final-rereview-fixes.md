# Task 22 Final Rereview Fixes Implementation Plan

> **For Codex:** Execute this plan task-by-task with `superpowers:executing-plans`. Keep each production change behind a failing regression, use a disposable PostgreSQL 18 container for database-backed tests, and never stage the pre-existing untracked `target/` directory.

**Goal:** Close every P1/P2 finding in `task-22-final-rereview.md` without regressing the Task 22 bootstrap/backlog behavior.

**Architecture:** The storage health probe will perform the same hard-link installation across private staging and object directories as real promotion. Telemetry will split trace export policy from log filtering, retain observable live-metric instruments with invalidatable samples, and run worker sampling independently of job execution. Consistency checking will separately inventory every lifecycle state that owns a physical file while applying integrity checks only to ready blobs. CLI storage checks will load the same validated storage settings as the runtime and reject the removed staging-root variable explicitly.

**Tech Stack:** Rust 1.90, Tokio, Axum, OpenTelemetry SDK 0.31, SQLx/PostgreSQL 18, Compose, clap, local filesystem storage.

---

### Task 1: Prove object promotion capability in readiness checks

**Files:**
- Modify: `crates/storage-local/src/operations.rs`
- Test: `crates/storage-local/tests/storage_contract.rs`

**Step 1: Write the failing regressions**

Add Unix-only tests which create an object directory with an unsafe/unusable mode and assert `probe_write()` fails. Keep a success test proving the probe removes both staging and object probe artifacts.

**Step 2: Run the focused tests and confirm failure**

Run: `cargo test -p folioharbor-storage-local --test storage_contract probe`

Expected: the unusable-object-directory case passes unexpectedly under the old staging-only probe.

**Step 3: Implement the minimum promotion-equivalent probe**

Create a private staging probe file, fsync it, hard-link it into a reserved private directory under `objects`, validate/open the installed file, unlink both paths, and fsync both parent directories. Ensure best-effort cleanup runs on every error path and verify private permissions on every participating directory.

**Step 4: Run focused and crate tests**

Run:
- `cargo test -p folioharbor-storage-local --test storage_contract probe`
- `cargo test -p folioharbor-storage-local`

Expected: PASS.

### Task 2: Preserve W3C spans when logs are filtered

**Files:**
- Modify: `crates/http/src/middleware/telemetry.rs`
- Test: `crates/http/tests/telemetry.rs`
- Test: `apps/worker/tests/worker_provenance.rs`

**Step 1: Write a production-subscriber regression**

Build the subscriber through the same helper used by `init_observability`, set the log filter to `warn`, use the in-memory OpenTelemetry span exporter, issue a traced request, and assert the response trace header and exported W3C IDs are nonzero and equal.

Extend the worker propagation test to run its span through that same warn-filter subscriber and assert the linked request trace ID survives.

**Step 2: Run focused tests and confirm failure**

Run:
- `cargo test -p folioharbor-http --test telemetry warn_filter`
- `cargo test -p folioharbor-worker --test worker_provenance warn_filter`

Expected: no request/worker spans are exported because the old global `EnvFilter` disables info spans.

**Step 3: Scope filtering to the log layer**

Extract a subscriber builder used by production initialization and tests. Attach the OpenTelemetry layer directly to the registry, and apply `EnvFilter` only to the JSON formatting layer so trace correlation spans remain available independently of log verbosity.

**Step 4: Re-run telemetry tests**

Run:
- `cargo test -p folioharbor-http --test telemetry`
- `cargo test -p folioharbor-worker --test worker_provenance`

Expected: PASS, including durable request/worker W3C identity assertions.

### Task 3: Retain live metrics and drain worker jobs on shutdown

**Files:**
- Add: `apps/worker/src/runtime.rs`
- Modify: `apps/worker/src/lib.rs`
- Modify: `apps/worker/src/main.rs`
- Test: `apps/worker/tests/runner.rs`

**Step 1: Write paused-time regressions**

Add a dispatcher that records active/max-active counts and blocks on a notification. Start one worker batch plus the production periodic reporter, advance paused time beyond multiple 15-second ticks, and assert the job remains active exactly once and the reporter continues sampling. Add a shutdown regression which signals termination while the dispatcher is blocked, asserts the worker has not returned, releases the dispatcher, and then observes a clean exit.

**Step 2: Run focused tests and confirm failure**

Run: `cargo test -p folioharbor-worker --test runner live_metrics -- --nocapture`

Expected: the old main-loop scheduling is not available as a retained/testable runtime and cannot satisfy the regression.

**Step 3: Implement retained scheduling and draining**

Move the periodic reporter into a separately retained Tokio task. Add a small runtime primitive that races shutdown with an in-flight iteration but, on shutdown, awaits that exact iteration before returning. Use it in `main`, stop leasing after shutdown wins, retain the existing batch semaphore for the whole batch, abort/await the reporter only after the active iteration is drained, and document that the Compose stop grace period is the outer hard bound.

**Step 4: Run worker tests**

Run:
- `cargo test -p folioharbor-worker --test runner`
- `cargo test -p folioharbor-worker`

Expected: PASS; maximum active dispatches never exceed configured concurrency and shutdown does not detach/cancel the job.

### Task 4: Model storage gauge invalidation

**Files:**
- Modify: `crates/http/src/middleware/telemetry.rs`
- Modify: `apps/api/src/main.rs`
- Modify: `apps/worker/src/main.rs`
- Test: `crates/http/tests/telemetry.rs`

**Step 1: Write collection-level regressions**

Use an SDK meter provider and in-memory exporter to collect the real `folioharbor.storage.free` observable gauge after a successful sample, a changed successful sample, and a failed sample. Assert the first two collections expose their current value and the failed collection exposes no stale point.

**Step 2: Run focused test and confirm failure**

Run: `cargo test -p folioharbor-http --test telemetry free_storage_gauge`

Expected: the old cumulative synchronous gauge retains the last successful value after sampling failure.

**Step 3: Add a retained operational-metrics sampler**

Register the live gauges once, retain their instrument handles, store the latest sampled state, and use an observable callback which emits free-space only while its sample is valid. Mark the sample invalid whenever `free_bytes()` fails. Construct one sampler per API/worker process and retain it for process lifetime.

**Step 4: Re-run telemetry and binary tests**

Run:
- `cargo test -p folioharbor-http --test telemetry`
- `cargo test -p folioharbor-api`
- `cargo test -p folioharbor-worker`

Expected: PASS.

### Task 5: Separate physical ownership from ready-blob integrity

**Files:**
- Modify: `crates/application/src/ports/consistency.rs`
- Modify: `crates/application/src/use_cases/check_storage_consistency.rs`
- Modify: `crates/postgres/src/repositories/consistency.rs`
- Modify tests for application consistency fakes
- Add or modify PostgreSQL/local-store integration tests

**Step 1: Write real PostgreSQL + local-store lifecycle regression**

Create canonical physical files and database locations in `ready`, `quarantined`, `purge_pending`, `deleting`, and `purged` states. Assert ready is integrity-checked; quarantined/purge-pending/deleting suppress false orphan findings without requiring integrity reads; and purged does not suppress an actual leftover object, which is reported as orphaned.

**Step 2: Run focused test against PostgreSQL 18 and confirm failure**

Start a uniquely named disposable PostgreSQL 18 container and run the focused integration test with `DATABASE_URL`.

Expected: the old ready-only repository marks all non-ready retained files as orphaned.

**Step 3: Introduce explicit inventory semantics**

Return all normal physical-owner lifecycle rows with a boolean/enum indicating whether integrity is required. Build the known-key set from ready, quarantined, purge-pending, and deleting rows; perform existence/hash/size checks only for ready; exclude purged rows so a lingering purged file remains an orphan finding.

**Step 4: Re-run application and PostgreSQL tests**

Run:
- `cargo test -p folioharbor-application storage_consistency`
- `DATABASE_URL=... cargo test -p folioharbor-postgres --test operations storage_consistency`
- `cargo test -p folioharbor-postgres`

Expected: PASS.

### Task 6: Reject the legacy storage variable and unify CLI defaults

**Files:**
- Modify: `crates/config/src/lib.rs`
- Modify: `crates/config/tests/settings.rs`
- Modify: `apps/cli/src/check_storage.rs`
- Test: `apps/cli/tests/admin_bootstrap.rs`

**Step 1: Write failing configuration regressions**

Assert that the exact removed `FOLIOHARBOR_STORAGE_STAGING_ROOT` variable causes validation failure. Assert the CLI storage-check composition resolves no-override configuration to `/var/lib/folioharbor` through the shared config loader rather than its former `/var/lib/folioharbor/blobs` fallback.

**Step 2: Run focused tests and confirm failure**

Run:
- `cargo test -p folioharbor-config rejects_legacy_storage_staging_root`
- `cargo test -p folioharbor-cli --test admin_bootstrap storage_check_default_root`

Expected: the old code ignores the legacy variable and returns the divergent CLI default.

**Step 3: Implement shared storage-only loading**

Reject the legacy environment key before environment mapping. Expose a storage-only configuration loader which uses the same raw defaults, source precedence, root validation, and staging derivation as full settings without requiring unrelated secrets. Make the CLI storage check call it.

**Step 4: Re-run config and CLI tests**

Run:
- `cargo test -p folioharbor-config`
- `cargo test -p folioharbor-cli`

Expected: PASS.

### Task 7: Full gates, Compose validation, report, and commit

**Files:**
- Modify ignored report: `.superpowers/sdd/2026-08-05-first-epub-vertical-slice/task-22-report.md`

**Step 1: Run formatting and static gates**

Run:
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --all-targets`

Expected: PASS.

**Step 2: Run real PostgreSQL and Compose gates**

Run database-backed integration tests against PostgreSQL 18, then:
- `docker compose config`
- `docker compose up -d --build`
- wait for healthy API/worker/PostgreSQL
- issue readiness/liveness checks
- inspect logs for migration/storage/telemetry failures
- `docker compose down -v --remove-orphans`

Expected: healthy services and clean shutdown within the configured grace period.

**Step 3: Inspect the diff and update the ignored report**

Document each rereview finding, its regression, exact gate results, Compose evidence, and any residual risk. Confirm `target/` remains untracked and unstaged.

**Step 4: Commit corrective changes**

Stage only intended source/test/plan files, commit with a Task 22 corrective message, and report the commit hash. Do not push.
