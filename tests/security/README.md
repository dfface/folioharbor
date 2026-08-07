# Security release gates

Security properties are enforced at several boundaries rather than by one broad integration test:

- `crates/postgres/tests/rls_matrix.rs` proves tenant isolation and least-privilege runtime roles against PostgreSQL 18.
- `crates/epub/tests/malicious_epub.rs` and the Playwright permissions journey reject unsafe archive paths and active content.
- `web/e2e/permissions.spec.ts` proves role boundaries, anti-enumerating wrong-library responses, immediate revocation, and response/log redaction.
- `web/e2e/upload-read.spec.ts` scans successful catalog, manifest, resource, progress, permission, and Range-download surfaces as well as denied responses and service logs.
- `web/e2e/security-harness.spec.ts` injects random sentinel/storage-key markers and proves the scanner fails with a fixed, non-echoing diagnostic without rejecting the legitimate reader CSP.
- authentication, CSRF, cookie, rate-limit, download, audit, and request-correlation contracts remain covered by the Rust HTTP and PostgreSQL suites.
- CI runs `cargo deny`, the production-only pnpm audit, and Gitleaks before the container smoke gate.

## Sensitive diagnostic policy

Never upload or paste raw Compose logs, request/response bodies, browser traces, videos, screenshots, Mailpit messages, database dumps, or generated EPUBs from a failed security/E2E run. Those surfaces can contain cookies, one-time tokens, user/library identifiers, publication content, local paths, storage keys, or hashes. The CI diagnostic artifact is deliberately reduced to service name, lifecycle state, health, and exit code.

The Playwright scanner is seeded with each journey's actual passwords, verification/invitation tokens, session and CSRF cookies, EPUB SHA-256 value, database storage key, and storage path. It checks raw and encoded variants across relevant successful and denied response headers/bodies and API/Worker logs. It also rejects cookie headers, database URLs, absolute storage paths, and storage/hash field markers. Captures stay in memory, and a match throws only the fixed message `E2E redaction gate detected sensitive data`; raw captures are never passed to a matcher or emitted in the diagnostic.

Before release, manually review one fresh failed-test artifact to confirm the CI artifact allowlist still holds. A scanner or artifact-policy failure blocks release even when functional tests pass.

This release does not claim penetration testing, third-party security certification, OIDC, remote object storage, or malware scanning. Those remain future work and must not be inferred from these gates.
