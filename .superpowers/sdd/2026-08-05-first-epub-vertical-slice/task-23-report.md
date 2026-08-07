# Task 23 report: complete vertical-slice release gates

## Outcome

Task 23 is implemented and every P1/P2 finding from the Task 23 review is closed. A clean PostgreSQL 18 + API + Worker + Mailpit + local Blob-store topology now drives 16 Playwright scenarios. One scenario is the required production-service browser acceptance with three isolated Chromium contexts; four are intentionally mocked reader-browser tests, nine are API/process journeys, and two exercise the fail-closed redaction harness. The categories are reported separately and the mocked tests are not represented as acceptance evidence.

The migration checker now proves an upgrade from the committed Task 23 base without checksum drift, a zero-to-28 migration with exact metadata/final-schema assertions, password-authenticated least-privilege roles, negative cross-credential probes, RLS, production API persistence, and Worker recovery. CI retains the required `rust`, `web`, `postgres`, `e2e`, `supply-chain`, and `container-smoke` jobs with explicit dependencies. CI artifacts are restricted to service name, lifecycle state, health, and exit code.

The release checklist and project documentation describe the implemented EPUB slice, operator checks, first-release migration-fixture policy, diagnostic restrictions, and explicitly excluded future capabilities.

## RED to GREEN evidence

The release journeys exposed production defects rather than being satisfied with mocks:

1. Linux storage readiness initially returned 503. The focused descriptor regression failed with `Bad file descriptor` because capability directories were opened with `O_PATH`, which cannot be synced or locked. Reopening `.` through the directory capability produced a normal directory file descriptor; the regression and the full storage suite pass.
2. The real mail journey initially failed because SMTP required STARTTLS while the test relay had no TLS identity, and then failed after a restart because a one-shot initializer rotated the CA underneath a running relay. The Compose topology now generates an idempotent private CA/server identity and lettre loads the native trust store. The registration/invitation journey passes without weakening STARTTLS.
3. The first real progress write returned `manifestation_not_found`: the Web client generated a device UUID but no production route enrolled it. The PostgreSQL adapter now atomically enrolls a first-seen Web device, updates last-seen state, and still refuses a revoked device. The focused PostgreSQL concurrency test and browser journeys pass.
4. The shared-Blob lifecycle journey left a Blob permanently reachable after both Items were purged. A focused catalog test showed one stale `installed_shared` reachability candidate instead of zero. Both definitions of `catalog_finish_import` now delete the temporary promotion guard in the same successful catalog transaction; the focused test and real GC journey pass.
5. Killing the API during an active request body left the upload in `receiving`. The recovery service already knew how to expire the durable receipt, but the production Worker never invoked it. The expiry handler now reconciles upload receipts before broader cleanup. Worker unit tests and the process-loss browser journey pass.
6. The GC failure injection originally changed only the object-store root mode; shard directories remained writable, so deletion succeeded and the recurring job returned to `pending`. Making every object directory read-only produced the intended `retry_wait|garbage_collection_unavailable`, retained the physical Blob, and then removed it after permissions and the retry schedule recovered.
7. `scripts/check-migrations.sh` initially exited before Cargo because `pg_isready` observed PostgreSQL 18's temporary Unix-socket bootstrap server immediately before its intentional restart. Waiting for the final TCP listener removed the race; the complete migration gate passes.
8. Clippy rejected `expect()` calls in the new descriptor test and then rejected the enlarged Worker `main`. The test now propagates errors, and runtime composition was extracted into a focused handler builder. Clippy passes without new lint suppression.
9. The production pnpm audit found high-severity `GHSA-qwww-vcr4-c8h2` through `react-router-dom` 7.18.2. The legacy compatibility package had no patched release, while the patched browser package was available as `react-router` 8.3.0. Imports and the direct dependency were migrated; audit, frozen install, build, and Web tests pass with no advisory exception.
10. The review's committed-base upgrade reproduced SQLx's `migration 10 was previously applied but has been modified` failure. Migrations 0010 and 0026 were restored byte-for-byte to the Task 23 base (SHA-256 `8872715d62cd2f49dde36f4a319ed1cb4f97a7f725dfb72d07e805111f5f7aad` and `b017424faa60c08314c7d01501ff2e05241150d58054753692f9e11736f3b708`), and the reachability fix moved to additive migration 0028. The checker now materializes the committed base migration tree with Git, applies it, upgrades with the current tree, and compares the stored/current checksums for versions 10 and 26.
11. The cross-role authentication regression initially showed that the API role accepted the Worker credential under `trust`. Test support, the migration checker, and E2E Compose now use distinct password-authenticated owner/API/Worker credentials. Compose passes only secret files, builds permission-restricted role URLs in a named volume, and blocks startup unless matching credentials work and mismatched API/Worker/owner credentials fail.
12. The original journeys passed raw response bodies/logs to negative matchers. The replacement gate scans actual application-user and infrastructure passwords, mail tokens, session/CSRF cookies, EPUB hashes, database storage keys, and storage paths in relevant successful/denied response headers/bodies and API/Worker logs. It checks encoded variants, keeps captures in memory, and emits only one fixed diagnostic. An injected-sentinel harness proved the failure path; a real reader CSP then exposed and fixed an over-broad `blob:` false positive without weakening opaque storage-key detection.
13. The real Chromium journey first exposed that session listing/revocation queried RLS tables without authenticated transaction context. A PostgreSQL regression failed with zero visible sessions, then passed after both operations applied `api_without_library` context in a transaction. The same browser journey subsequently exposed a one-chapter fixture that could not advance and a UI download link targeting nonexistent `/original`; the two-spine EPUB fixture and production `/download` link now have focused regressions.
14. The review's P2 gaps now have direct coverage: an editor completes a valid EPUB upload through Ready/Item/catalog visibility before management denial is checked; schema metadata asserts exactly one `singleton=true` row, exact version 28, a migration-window timestamp, representative final columns, and seeded reader permissions; every E2E account receives a fresh random password rather than a shared fixture constant.

## Implemented gates

- Playwright creates random per-run infrastructure secrets and per-account passwords, builds the release binaries, starts clean named volumes, bootstraps an administrator through the CLI, exercises real STARTTLS mail, requires PostgreSQL password authentication, and always records the allowlisted status before teardown.
- The production-service browser acceptance uses three isolated Chromium contexts and no API interception. It proves personal-library preservation, invitation acceptance through the UI, EPUB UI upload/import/read, cross-device chapter progress, default download denial, explicit reader-download enablement, browser Range semantics, and original-byte equality.
- Authorization journeys cover a complete editor upload, reader denial, unrelated and wrong-library anti-enumeration, malicious EPUB rejection, next-request revocation, and fail-closed scanning of successful/denied responses and service logs for actual sensitive values.
- Recovery journeys make real API and Worker process cuts, test simultaneous quota reservations and dedup scopes, and prove shared Blob and GC retry invariants.
- The PostgreSQL checker proves immutable-base upgrade, migration from zero, migration idempotency/exact schema metadata 28, password authentication, least-privilege/RLS behavior, production API persistence, and Worker restart recovery on PostgreSQL 18.
- CI runs formatting, Clippy, Rust/Web tests, generated OpenAPI diff, PostgreSQL checks, Playwright, Cargo deny, production pnpm audit, Gitleaks, an application image build, and official-Compose startup. `e2e` depends on `rust`, `web`, and `postgres`; `container-smoke` depends on `e2e` and `supply-chain`.

## Fresh verification

All required local matrix commands exited 0 on the final implementation:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features` against a fresh PostgreSQL 18 container
- `cargo deny check` (`advisories ok, bans ok, licenses ok, sources ok`)
- `pnpm --dir web lint`
- `pnpm --dir web typecheck`
- `pnpm --dir web test -- --run` (6 files, 68 tests)
- `pnpm --dir web exec playwright test` (16 passed in 4.1 minutes from clean volumes: 1 real acceptance, 4 mocked reader-browser, 9 API/process, 2 redaction harness)
- `docker compose -f deploy/compose.yaml config --quiet`
- `scripts/check-migrations.sh` on PostgreSQL 18, including committed-base checksum upgrade and wrong-password probes

Additional release evidence passed during the original Task 23 implementation and remains unchanged by this review repair:

- `pnpm install --frozen-lockfile --offline`
- `pnpm web:check-generated-api`
- `pnpm audit --prod --audit-level high` (`No known vulnerabilities found`)
- `pnpm --dir web build`
- a real official-Compose smoke under an isolated project: `api`, `postgres`, and `worker` healthy; `migration` and `storage-init` exited 0; CLI bootstrap and `/health/ready` succeeded
- CI YAML parsing plus exact job/dependency assertions
- `git diff --check`

## Environmental caveats

- One intermediate Docker rebuild failed before application compilation with a TLS handshake EOF while contacting `static.rust-lang.org`. This was an external toolchain-network failure, not an application or container-readiness failure. The E2E Dockerfile now selects the exact toolchain already installed in `rust:1.88-bookworm`, avoiding an unnecessary rustup channel sync; later clean image builds passed.
- Vitest emits jsdom/axe `HTMLCanvasElement.getContext` “not implemented” diagnostics. They are pre-existing test-environment noise; all 67 tests pass and the real Chromium reader scenarios also pass.
- Gitleaks is not installed on this workstation. The workflow contains the required full-history Gitleaks job, but that particular external action must be confirmed by CI for the release commit. No local success is claimed for it.
- No previous-release schema fixture was fabricated. The release checklist requires adding a provenance-backed supported-version fixture after the first production tag and before the next release.

## Scope and handoff

No future-scope capability was added. The untracked root `target/` directory predated this task and remains unstaged. No remote push was performed.
