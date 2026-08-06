# Task 17 transactional mail pipeline report

## Status

DONE. Verification, invitation, and password-reset flows now seal an encrypted mail intent and commit it in the same PostgreSQL transaction as their business mutation. The production worker leases those intents, decrypts only for one attempt, renders localized text/HTML, and sends through a TLS-enforcing SMTP adapter. No push was performed.

## Delivered architecture

- The API composition root constructs one `MailOutbox` sealer from the application secret ring and injects it into identity and library services; it no longer injects the failure-only direct mailer.
- Registration uses `register_with_verification`, reset uses `issue_password_reset_with_mail`, and invitation uses `create_invitation_with_mail`. Each PostgreSQL adapter owns one transaction containing both business state and the outbox row; invitation also includes the allowed audit row.
- Unknown password-reset identities retain the same public result and execute the sealing work, but discard the dummy intent instead of persisting it.
- `mail_outbox` persists recipient/account context, template/version/locale, AES-256-GCM ciphertext, key ID, a 96-bit OS-random nonce, token-free idempotency key, invitation library/role context, attempts, scheduling, leases, expiry, and terminal timestamps.
- AEAD associated data binds template code/version, locale, recipient account ID, normalized address, invitation library ID, and role.
- Worker leasing uses `FOR UPDATE SKIP LOCKED`, increments attempts, recovers abandoned leases, schedules bounded retries, and enforces lease ownership on every transition.
- Sent, permanently failed, and expired rows erase `token_ciphertext`. Rendered subjects/bodies have no `Debug` implementation and zeroize on drop; the raw SMTP buffer is zeroized after each attempt.
- English and Simplified Chinese templates produce plain and HTML alternatives. Chinese invitation context labels are localized, untrusted context is escaped, and templates contain no tracking or remote resources.
- Link rendering rejects non-HTTP URLs and any scheme/host/effective-port that differs from the validated public base URL.
- The worker composition root loads validated settings, creates `PgMailRepository`, `DeliverMailJob`, and `SmtpMailer`, and polls bounded mail batches alongside import/cleanup work.

## SMTP and availability behavior

- `smtp://` requires STARTTLS; a relay that does not advertise it is rejected before credentials or message data are sent. `smtps://` uses implicit TLS. Production retains certificate and hostname validation.
- SMTP commands have a ten-second timeout. Username/password must be configured together, and error/debug surfaces contain only static safe codes.
- 4xx/transport failures retry; 5xx responses are terminal. One intent retains one stable token-free Message-ID/idempotency key across retries.
- API startup validates that enabled mail flows have valid SMTP configuration but does not contact SMTP. A transient SMTP outage therefore affects only worker retry state, not existing reading traffic.
- `mail.from_address` / `FOLIOHARBOR_MAIL_FROM_ADDRESS` is validated and documented in the deployment example.

## TDD evidence

1. Registration atomicity RED: `cargo test -p folioharbor-postgres --test mail_pipeline registration_and_verification_intent_commit_or_roll_back_together` failed to compile because the combined operation/outbox fields did not exist. GREEN asserts `(account, intent)` is `(1,1)` on success and `(0,0)` when the intent violates the schema.
2. Reset atomicity RED: its test failed to compile without recipient lookup and combined reset/intent persistence. GREEN asserts reset token and intent commit or roll back together.
3. Invitation atomicity RED: its test failed without the combined invitation/audit/mail operation. GREEN asserts all three rows commit together and all three roll back at the invalid-intent cut.
4. Worker persistence RED: the transition test initially returned the repository port's unimplemented error. Real-database REDs then exposed missing worker RLS context and a contradictory terminal-ciphertext check. GREEN covers retry timing, stable idempotency, sent/failed/expired erasure, attempt counts, and abandoned-lease recovery.
5. API composition RED: `registration_uses_the_combined_account_and_outbox_repository_operation` failed to compile before `MailIntentSealer`. GREEN proves selection of the combined operation.
6. Delivery RED: `transient_retry_reuses_idempotency_key_then_marks_terminal_success` failed to compile before `DeliverMailJob`, lease types, and terminal transitions. GREEN proves key reuse and later success.
7. Configuration RED: partial SMTP credentials and invalid from-address settings were accepted. GREEN rejects both without echoing sentinel values.
8. SMTP policy RED: the worker test failed to compile before `SmtpMailer`/`SmtpSecurity`. GREEN proves required STARTTLS, implicit TLS, configured ports, and the ten-second timeout.
9. Origin/localization RED: tests failed against the old renderer and English-only invitation labels. GREEN rejects mismatched origins and verifies Chinese labels plus escaping.

## PostgreSQL evidence and environment limitation

The PostgreSQL tests require `FOLIOHARBOR_TEST_DATABASE_URL`; shells without it cannot exercise the adapter. This repair used a local PostgreSQL 18 Docker container named `folioharbor-task17-pg`, with:

```text
FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres
```

Fresh focused results:

```text
cargo test -p folioharbor-postgres --test mail_pipeline -- --nocapture
4 passed; 0 failed

cargo test -p folioharbor-postgres --test migration_from_zero migrations_from_zero_preserve_least_privilege_roles_and_are_idempotent -- --exact --nocapture
1 passed; 0 failed
```

The migration test confirms versions 1 through 24 apply from zero, preserve least-privilege roles, and remain idempotent. The first API-role insert also exposed and drove the required `INSERT`/`SELECT(idempotency_key)` grants and matching RLS selection policy.

## Local STARTTLS capture evidence

Mailpit was initially run on ports `58025/58080`; its default plaintext listener did not advertise STARTTLS. It was restarted with an ephemeral localhost certificate and `--smtp-require-starttls`. The ignored capture test uses required STARTTLS, with certificate verification disabled only inside that test for the ephemeral self-signed capture certificate; production does not disable verification.

```text
FOLIOHARBOR_SMTP_CAPTURE_PORT=58025 cargo test -p folioharbor-worker --lib starttls_capture_receives_multipart_message -- --ignored --nocapture
1 passed; 0 failed
```

Mailpit decoded one message with sender/recipient, a stable token-free Message-ID, plain and HTML parts containing the identical link, zero attachments, and no remote content. Test output was scanned with `rg` for the token and credential sentinel; neither appeared. The Mailpit API payload was separately asserted with `jq` to contain the expected link in both `.Text` and `.HTML`.

## Verification

Fresh final non-database checks:

```text
cargo fmt --all -- --check
git diff --check
cargo check --workspace --all-targets
cargo test -p folioharbor-application --test mail_delivery
# 7 passed; 0 failed
cargo test -p folioharbor-worker --test smtp_transport
# 1 passed; 0 failed
```

The full workspace gate was also run against the same PostgreSQL 18 URL:

```text
FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres cargo test --workspace --all-targets --all-features
```

It completed with zero failures. The external Mailpit test is explicitly ignored in ordinary workspace runs and was executed separately as shown above.

## Security self-review

- No plaintext token or full single-use URL is persisted or logged.
- Idempotency keys and SMTP error codes contain no token material.
- GCM nonces come directly from the OS CSPRNG and are stored in full.
- Owned key/plaintext/token/rendered/raw buffers are explicitly zeroized where controlled by this code; terminal rows erase ciphertext.
- The API role cannot lease/transition mail, and the worker role cannot perform API business mutations.
- Production SMTP never downgrades to plaintext and does not weaken certificate verification.

## Commit

All Task 17 repair files and this report are included in `feat: deliver transactional account and invitation email`.

## Final-review follow-up

The four P1 findings and the standalone enqueue P2 contract are repaired in this follow-up:

- A single problem registry now owns every production-emitted public type slug and its documentation category. Type-URI construction resolves through that registry, unknown public document slugs remain 404, and the route negotiates only `en`/`zh-CN`, including explicit `q=0`, wildcard, unsupported-leading, and fallback behavior. The route test requests every registered emitted slug, including JSON-rejection and upload/library validation codes.
- The complete Lettre `send_raw` future is enclosed by the ten-second Tokio deadline. Timeout produces the static transient code `smtp_exchange_timeout`, and a paused-time TCP fixture proves a peer that accepts but never greets cannot hang delivery.
- Retry, sent, and failed transitions validate both owner and lease/intent expiry against PostgreSQL `clock_timestamp()`. Retry delay is anchored to database completion time. Real-database coverage proves all transitions reject a lease that expired after a stale batch start, an expired intent is rejected, and a wrong owner cannot transition a fresh lease.
- `MailMode` is derived once from the registration/verification/invitation/reset flags in shared configuration. Shared parsing rejects opaque SMTP URLs and any relay URL without a usable authority. Production API route composition removes disabled mail-producing routes while retaining reading/library routes; the worker constructs and polls mail delivery only when the same mode is enabled. Readiness is a pure validated-configuration property and never probes a transiently unavailable relay.
- `PgMailRepository::enqueue` now returns the actual stored UUID after an idempotency conflict through a narrowly granted security-definer lookup, retaining the API role's column-level/RLS restrictions.
- Automated content evidence now covers all three templates in both locales, exactly one identical single-use link in each text/HTML alternative, no remote content, and stable raw MIME/Message-ID across retry.

### Follow-up RED/GREEN evidence

1. Problem registry RED: `cargo test -p folioharbor-http --test problem_documents -- --nocapture` failed to compile because `problem_document_router` did not exist. GREEN: 2 passed, 0 failed; the existing problem contract also reports 4 passed.
2. Complete SMTP deadline RED: the stalling-peer test reached its eleven-second caller guard with `Elapsed(())`. GREEN: `cargo test -p folioharbor-worker --test smtp_transport` reports 3 passed, including disabled mode and the stalling peer.
3. Enqueue return RED: the duplicate-key real-PostgreSQL assertion received the second, unstored UUID. GREEN: it returns the first stored UUID and finds exactly one row.
4. Database-time lease RED: `mark_sent` accepted a lease already expired relative to database time. GREEN: all three transitions, intent expiry, and wrong-owner cases reject invalid ownership/time.
5. Shared mail mode RED: the configuration tests failed to compile because `MailSettings` had no mode/readiness surface; the worker test likewise failed because `SmtpMailer::for_mode` did not exist, and the API route test failed because `AppState::with_mail_mode` did not exist. GREEN: configuration reports 19 passed, auth routes 11 passed, library routes 4 passed, and worker SMTP tests 3 passed.

### Fresh follow-up verification

Using PostgreSQL 18 at `FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres`:

```text
cargo fmt --all -- --check
# exit 0

git diff --check
# exit 0

cargo check --workspace --all-targets
# exit 0

cargo test -p folioharbor-postgres --test mail_pipeline -- --nocapture
# 6 passed; 0 failed

cargo test -p folioharbor-postgres --test migration_from_zero migrations_from_zero_preserve_least_privilege_roles_and_are_idempotent -- --exact --nocapture
# 1 passed; 0 failed

cargo test -p folioharbor-application --test config_contract --test mail_delivery -- --nocapture
# config: 19 passed; mail delivery: 7 passed; 0 failed

cargo test -p folioharbor-http --test problem_documents --test problem_contract --test auth_routes --test library_routes -- --nocapture
# problem documents: 2; problem contract: 4; auth: 11; libraries: 4 passed; 0 failed

cargo test -p folioharbor-worker --test smtp_transport --lib -- --nocapture
# SMTP transport: 3 passed; library: 2 passed, 1 ignored; 0 failed

cargo test --workspace --all-targets --all-features --quiet
# exit 0; all executed tests passed; one external STARTTLS capture test remained intentionally ignored
```
