# Security release gates

Security properties are enforced at several boundaries rather than by one broad integration test:

- `crates/postgres/tests/rls_matrix.rs` proves tenant isolation and least-privilege runtime roles against PostgreSQL 18.
- `crates/epub/tests/malicious_epub.rs` and the Playwright permissions journey reject unsafe archive paths and active content.
- `web/e2e/permissions.spec.ts` proves role boundaries, anti-enumerating wrong-library responses, immediate revocation, and response/log redaction.
- authentication, CSRF, cookie, rate-limit, download, audit, and request-correlation contracts remain covered by the Rust HTTP and PostgreSQL suites.
- CI runs `cargo deny`, the production-only pnpm audit, and Gitleaks before the container smoke gate.

## Sensitive diagnostic policy

Never upload or paste raw Compose logs, request/response bodies, browser traces, videos, screenshots, Mailpit messages, database dumps, or generated EPUBs from a failed security/E2E run. Those surfaces can contain cookies, one-time tokens, user/library identifiers, publication content, local paths, storage keys, or hashes. The CI diagnostic artifact is deliberately reduced to service name, lifecycle state, health, and exit code.

Before release, manually review one fresh failed-test artifact to confirm the allowlist still holds. Also scan application responses and logs for `Cookie`, session values, plaintext mail tokens, absolute storage paths, opaque storage keys, and cross-library content hashes. A redaction failure blocks release even when functional tests pass.

This release does not claim penetration testing, third-party security certification, OIDC, remote object storage, or malware scanning. Those remain future work and must not be inferred from these gates.
