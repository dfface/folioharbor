# Whole-branch P1 repair report

**Review repaired:** `whole-branch-review.md`

**Scope:** all five P1 findings

**Outcome:** implementation complete; final production-container verification and commit evidence are recorded below.

## Repairs

### P1-1 — Production Web deployment

- Added one production multi-stage image at `deploy/Dockerfile`; it builds the Web bundle and Rust binaries and serves the SPA through an unprivileged nginx runtime.
- Added `deploy/nginx.conf` with same-origin API/health proxying, application deep-link fallback, immutable fingerprinted-asset caching, no-store HTML, and browser security headers.
- Added the `web` service to official Compose, made the API network-internal, and documented the Web-facing topology.
- Replaced the E2E-only Rust image and host Vite Playwright server with the production image/Web container.
- Added `web/e2e/deployment.spec.ts` and CI container-smoke checks for deep links, assets, caching, and security headers.
- The runtime remains non-root with a read-only root filesystem; nginx PID and all temporary paths use its writable `/tmp` mount. Test Compose uses the same internal API port as production while preserving the existing host API port.
- RED evidence: the former Vite path did not return the required production cache/security headers; the first production image builds also exposed registry/Corepack failure recovery gaps and downloaded test-only browser packages. The image now uses persistent BuildKit caches, bounded Corepack/pnpm retries, the trusted committed lockfile, and the 81-package production/build dependency set rather than the 342-package test graph. The first real Compose boots additionally exposed the stale schema-28 readiness gate and nginx's implicit root-owned FastCGI temporary directory; both failed before browser execution and are now covered by the schema-29 health contract and production-container smoke.

### P1-2 — CI migration history

- Configured the Rust and PostgreSQL jobs with `fetch-depth: 0` so the committed migration base is available wherever the ancestor-dependent regression can run.
- GREEN evidence: `scripts/check-migrations.sh` passes `committed_task_base_upgrades_without_sqlx_checksum_drift` before the remaining migration/runtime-role suite.

### P1-3 — Runtime storage policies

- The configured free reserve now reaches every production `LocalBlobStore`, so readiness and all write-capacity checks use the same threshold.
- Upload admission uses `storage.upload_limit`; the fixed Web/OpenAPI/database 1 GiB divergence was removed, and the server remains authoritative before content transfer.
- Personal-library provisioning persists `storage.library_quota` through a context-bound database contract.
- API/Worker repository composition now supplies configured failed-upload retention, item recovery period, and Blob GC delay. Migration 0029 supplies schema-compatible configured functions and removes the superseded fixed-duration entry points.
- Added signed-`bigint` configuration validation and non-default behavior tests for reserve, upload limit, quota, failed retention (including reconciliation), recovery, and GC.
- RED evidence: non-default reserve initially remained pinned to `MIN_FREE_BYTES`; migration reconciliation also exposed an ambiguous SQL output/table column and was fixed with a qualified predicate.

### P1-4 — Stale `SECURITY DEFINER` contracts

- Migration 0029 revokes and drops the superseded personal-library, invitation, library-read, lifecycle, import-retention, and GC function signatures.
- `list_members` and personal-library provisioning use guarded replacements bound to `session_user` and the transaction request/user/library context.
- The RLS matrix now attempts API execution with Alice's context and Bob's arguments, and asserts the obsolete functions are absent or non-executable.

### P1-5 — Account-scoped reader device identity

- Device IDs are stored under account-scoped v2 keys; existing v1 pending progress is migrated only for its owning account.
- Reset removes only the selected account's identity/queue.
- Unit coverage proves per-account identity and queue isolation. The same-browser Playwright journey now switches A → B → A and proves distinct device IDs, successful B writes, recovery of A's pending write, and preservation of both accounts' keys.

## Verification

- `cargo fmt --all --check` — PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — PASS.
- `cargo test --workspace --all-features` with disposable PostgreSQL 18 and runtime-role credentials — PASS (one STARTTLS/Mailpit-only test remains intentionally ignored by the suite).
- `scripts/check-migrations.sh` — PASS, including committed-base checksum upgrade, schema 29/idempotence, runtime credentials/RLS, configured policy paths, upload composition, and worker restart.
- `pnpm --dir web lint` — PASS.
- `pnpm --dir web typecheck` — PASS.
- `pnpm --dir web test -- --run` — PASS, 68 tests (the existing jsdom/axe canvas warnings remain non-failing).
- `pnpm --dir web run build` — PASS, Vite production bundle with 196 transformed modules.
- `docker compose -f deploy/compose.yaml config --quiet` — PASS.
- `docker compose -f tests/e2e/compose.test.yaml config --quiet` — PASS.
- `docker build -f deploy/Dockerfile -t folioharbor-e2e-app:local .` — PASS; final verified image `sha256:885e85c4697af44903fa5d4dc828e6dc69e3e49368d81bc14ac26be5838cbb2c`.
- `FOLIOHARBOR_E2E_SKIP_BUILD=1 pnpm --dir web exec playwright test` — PASS, all 17 browser tests against the production Web/API/Worker/PostgreSQL/Mailpit Compose topology in 4.0 minutes.

## Commit

Ready in the local repair commit. No push was performed.
