# Task 18 Report: Recoverable Item Deletion and Safe Blob GC

## Status

Implemented and committed as a single Task 18 change. Item deletion is immediately access-revoking and recoverable for exactly seven days; item purge releases logical quota and detaches rebuildable package data; physical blob deletion is a separately leased, retryable phase delayed by exactly 24 hours.

## TDD evidence

- RED: `cargo test -p folioharbor-application --test item_lifecycle` failed to compile before the lifecycle model, use cases, and ports existed.
- GREEN: the application lifecycle suite passes 3/3, covering exact seven-day and 24-hour boundaries, `holding.edit` plus allowed-audit propagation, and storage-failure claim release/retry.
- RED: `cargo test -p folioharbor-postgres --test blob_gc --no-run` failed to compile before the PostgreSQL lifecycle/GC repositories and contracts existed.
- GREEN with real PostgreSQL: the blob GC suite passes 4/4, covering atomic delete/restore and audit, quota/package/progress retention, shared references, and concurrent reference creation versus the final GC recheck.
- RED/GREEN follow-up: the elapsed recovery-window HTTP problem test first observed the generic internal-error type, then passed after registering and documenting `item-recovery-window-elapsed`.
- Migration-from-zero passes 2/2 after adding migration 0025 to the expected migration set.

## Implementation

- Added explicit `Active`, `Deleted`, `PurgeEligible`, and `Purged` item lifecycle states and valid timestamp combinations.
- Added authorized, locked, idempotent delete/restore mutations with membership-version revalidation and same-transaction allowed audit records.
- Added `DELETE /v1/libraries/{library_id}/items/{item_id}` and `POST .../restore`, CSRF enforcement, OpenAPI operations, and the public recovery-window problem.
- Added bounded `SKIP LOCKED` item preparation and blob claim phases. Preparation rechecks authoritative table references, releases quota, detaches item/package derivatives, and marks unreferenced locations purge-pending. Claiming rechecks references again, leases the location, deletes storage idempotently, then completes or releases the claim for retry.
- Reference guards lock the blob and reject new attachment once its location is `deleting` or `purged`; references during `purge_pending` make the final transactional recheck fail.
- Reading progress retains manifestation, content-unit, and locator identity while nullable package references use `ON DELETE SET NULL`; append-only audit rows survive item purge.
- Composed `CollectGarbage` in the worker while retaining the existing cleanup-only dispatcher fallback used by restart/recovery tests.
- Split the lifecycle-capable catalog service from the query-only service so existing read-only repository consumers do not acquire a write-side trait requirement.

## Verification

- `cargo fmt --all --check`: pass.
- `git diff --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `FOLIOHARBOR_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:55432/postgres cargo test --workspace --all-features`: pass; one STARTTLS/Mailpit test remains intentionally ignored by its existing test annotation.
- Focused real-PostgreSQL verification: item lifecycle 3/3, blob GC 4/4, migration-from-zero 2/2, and worker cleanup restart regression 1/1.
- `cargo deny check`: advisories, bans, and sources pass, but the license gate remains blocked because the current `lettre` dependency graph includes `quoted_printable` under `0BSD` and `webpki-roots` under `CDLA-Permissive-2.0`, neither currently allowed by the repository policy. Task 18 does not change dependencies or `Cargo.lock`.

## Global-gate compatibility fixes included

The parent authorized the smallest behavior-preserving repairs needed for the repository-wide strict Clippy gate: auth feature booleans are stored as a bitset, a redundant `must_use` was removed from a `Router` return, SMTP error classification borrows its error, the worker connect call avoids a needless borrow, and test-local Clippy allowances were added to the existing long/expect-heavy mail pipeline test.

## Concerns

- The repository license allowlist needs a separate policy decision for the two transitive `lettre` licenses above before `cargo deny check` can be green.
- No push was performed.

## Independent-review fixes

All P1/P2 findings in `task-18-review.md` were addressed with focused RED/GREEN regressions:

- Post-purge DELETE: a real authorized PostgreSQL delete after GC initially returned an application error; it now returns the existing `Purged` lifecycle and records another allowed delete audit. OpenAPI no longer advertises a recovery-window conflict for DELETE, while restore retains 409.
- Lock order: a deterministic real-PostgreSQL race first reproduced `deadlock detected` by queuing GC and `catalog_validate_import` behind a Blob blocker while import held Library. GC now locks Library before Blob, matching import/quota order; the same test completes both transactions.
- Terminal delay: a direct invalid `purged` row with a zero-hour pending interval was initially accepted. The terminal constraint now requires `purge_after = purge_pending_at + 24 hours`; legacy purged rows and legacy transitions are explicitly represented by backdating `purge_pending_at` 24 hours.
- Lease fencing: after same-owner expiry/reclaim, stale release initially changed the successor lease. Every claim now receives a fresh UUID fencing token returned through `BlobPurgeClaim`; completion and release match owner plus token, so stale completion/release return false and fresh claims complete.

Review-fix verification:

- `FOLIOHARBOR_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:55432/postgres cargo test -p folioharbor-application --test item_lifecycle -p folioharbor-postgres --test blob_gc --test migration_from_zero -p folioharbor-http --test catalog_routes`: pass (3 + 8 + 2 + 4 tests).
- `cargo fmt --all --check`: pass.
- `git diff --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.

## Re-review fixes

Both P1 findings in `task-18-rereview.md` were addressed with focused RED/GREEN regressions:

- Multi-library batching: a real shared-Blob, two-library regression initially failed with `garbage collection persistence failed` after PostgreSQL detected the retained-lock deadlock. Preparation now commits at most one candidate per transaction while preserving the caller's total requested limit, releasing each Library/Blob lock set before selecting the next item.
- Re-import revival: real reconciliation against a matching purged content-addressed location initially failed with `import persistence is temporarily unavailable`. Ready and quarantined transitions now clear every purge lifecycle timestamp and lease field; the migration 0025 trigger also enforces that reset for upgraded schemas whose migration 0010 function already existed.

Re-review verification:

- `FOLIOHARBOR_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:55432/postgres cargo test -p folioharbor-postgres --test blob_gc --test import_cleanup --test migration_from_zero --test catalog_constraints`: pass (9 + 5 + 2 + 11 tests).
- `cargo fmt --all --check`: pass.
- `git diff --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.

## Final re-review fix

The remaining normal-upload revival gap from `task-18-final-rereview.md` was addressed with a real RED/GREEN path:

- RED: a normal instance-scoped upload created through `PgUploadRepository` and received through `UploadService` with `LocalBlobStore` failed during promotion when the content-addressed database location was already `purged`; reconciliation was never reached.
- GREEN: the candidate guard now permits the terminal `purged` location so the re-uploaded bytes are protected by the reachability candidate until reconciliation restores the existing location to `ready`. The guard still locks the Blob and rejects `deleting`, preventing an active GC lease from deleting newly promoted bytes.
- The regression proceeds through upload creation, receipt preparation, physical storage promotion, upload finalization, and worker reconciliation. It verifies the physical bytes, existing Blob reuse, and clearing of all purge lifecycle and lease fields. A companion real-PostgreSQL regression verifies that promotion against an active `deleting` location remains fenced.

Final re-review verification:

- `FOLIOHARBOR_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:55432/postgres cargo test -p folioharbor-postgres --test import_cleanup --test blob_gc --test upload_state_machine --test migration_from_zero --test catalog_constraints`: pass (7 + 9 + 4 + 2 + 11 tests).
- `cargo fmt --all --check`: pass.
- `git diff --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
