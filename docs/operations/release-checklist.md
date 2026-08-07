# First EPUB vertical-slice release checklist

Use this checklist from a clean clone on the exact commit proposed for the first tag. Record the commit SHA, command results, PostgreSQL image digest, application image digest, reviewer, and UTC timestamp in the release record. Any unchecked item blocks the tag.

## Automated gates

- [ ] The CI jobs `rust`, `web`, `postgres`, `e2e`, `supply-chain`, and `container-smoke` all passed for the same commit and their declared dependencies were not bypassed.
- [ ] The following matrix was also run fresh when reproducing a release locally:

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

- [ ] `scripts/check-migrations.sh` used PostgreSQL 18, migrated an empty database through every committed migration, reran migration idempotently, checked schema metadata, exercised API/Worker persistence, and passed the RLS matrix.
- [ ] Generated OpenAPI Web types are current (`pnpm web:check-generated-api`).
- [ ] Dependency vulnerability/license checks, the production pnpm audit, and the secret scan passed without an unreviewed suppression.
- [ ] The application image built from the release commit and the official `deploy/compose.yaml` reached its documented healthy state from empty volumes.

## Operator verification

- [ ] Generate new, independent PostgreSQL-role passwords and an application secret of at least 32 random bytes; place them in permission-restricted secret files. Do not reuse E2E or example values.
- [ ] Confirm owner, API, and Worker database URLs use only their matching role and that runtime roles are neither superusers nor `BYPASSRLS`.
- [ ] Confirm Blob storage is persistent, backed up with the database as one consistency boundary, owner-only (`0700`), writable by the configured runtime UID/GID, and has enough free space above the configured reserve.
- [ ] Run migrations with the owner credential before API/Worker rollout. Capture only migration versions and success state, never URLs or secret contents.
- [ ] Bootstrap exactly one system administrator through the CLI over a private terminal. Verify the password and any mail token are absent from retained shell history and logs.
- [ ] Configure the public base URL, TLS termination, SMTP sender, storage reserve/quota, log filter, and time synchronization. Do not expose Mailpit in production.
- [ ] Verify `/health/live` and `/health/ready`, then perform the two-user flow: registration, verification, invitation, EPUB upload/import, catalog/reader resource, cross-device progress, download denial, explicit download enablement, and byte-range hash match.
- [ ] Revoke the reader and confirm the next resource request is denied. Confirm unrelated and wrong-library identifiers receive anti-enumerating responses.
- [ ] Restart API and Worker during an expendable upload/import and verify the documented terminal state and retry behavior. Exercise one GC failure and recovery without deleting the last shared Blob owner.
- [ ] Review runtime logs and retained CI diagnostics for cookies, plaintext tokens, database URLs, local paths, storage keys, EPUB content, and cross-library hashes. Any disclosure blocks release.
- [ ] Test database-plus-Blob backup restoration in an isolated environment and verify catalog, resource, original download, and consistency-check behavior before declaring the backup usable.

## Migration fixture policy

This is the first release, so there is no honest supported previous-release schema fixture. Do not fabricate one. After the first production tag, export and commit the supported previous schema fixture, document its provenance, and enable previous-version-to-current upgrade testing before the next release candidate.

## Scope confirmation

- [ ] Release notes describe only local accounts, local-disk EPUB upload/import/read/download, collaboration roles/invitations, progress synchronization, quota, lifecycle/GC, operations, and the tested Compose deployment.
- [ ] OIDC, PDF/TXT reading, OPDS, annotations, S3-compatible storage, search, federation, mobile apps, and malware scanning remain explicitly labeled future work.
