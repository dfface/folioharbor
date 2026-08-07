# Task 21 Report: Secure EPUB Reader and Cross-Device Progress UX

## Status

Implemented the Task 21 reader vertical slice and closed every P1/P2 finding from the independent review. The Web application validates publication links against the selected Item's opaque resource route, renders publication bytes only in a privilege-free sandboxed Blob iframe, applies real continuous/paginated typography inside that isolated document, traps modal focus, and synchronizes account-scoped progress through explicit conflict choices.

The review repair extends the shared auth/progress contract. A current session now exposes its stable `user_id`; every progress mutation carries that value as `accountId`; and the server rejects a mutation whose account differs from the currently authenticated actor. This complements account-scoped client persistence and prevents a bounded unload request created under account A from being accepted with account B's later cookies.

## RED evidence

The original reader and progress suites were introduced before their production modules. Their initial runs failed on the missing route, isolated frame, navigation, settings, resource states, security validation, state machine, and lifecycle behavior.

The review repairs added regressions before implementation:

- five progress tests failed because pending data was not account-scoped, a delayed startup GET could overwrite newer accepted state, online recovery did not retry the initial GET, and conflict resolution discarded a newer queued Locator;
- three reader tests failed because background shortcuts remained active behind the TOC, Shift+Tab could escape dialogs, and flow/font settings did not alter publication layout;
- the HTTP account-mismatch test returned 422 instead of the required private 403, while the OpenAPI regression showed `accountId` was absent;
- the first real Chromium run caught deferred focus restoration while the background was still inert; the first broad Vitest run also proved the Playwright and unit-test collection boundaries needed to be separated.

Each of those failures was observed before its corresponding implementation change. Conflict tests continue to use a deliberately smaller device percentage so no implementation can pass by selecting the maximum percentage.

## Delivered behavior

- Nested `/libraries/:libraryId/items/:itemId/read` route in the authenticated library shell.
- Typed manifest, session, and progress DTOs generated from `openapi/folioharbor-v1.yaml`.
- Same-origin exact-Item resource allowlist with opaque resource IDs, no query strings, and validation before any publication fetch.
- Sanitized publication bytes loaded through revocable Blob URLs in `<iframe sandbox="">`; content is never inserted into the parent DOM.
- Real isolated-document layout: continuous vertical overflow or paginated CSS columns/horizontal snap, plus actual body font scaling and reduced-motion scroll behavior.
- Opaque-link TOC and previous/next navigation, arrow-key navigation, Escape close, focus containment in TOC/conflict dialogs, post-close focus restoration, and an inert/`aria-hidden` reader background while a modal is open.
- Readium Locator reporting independent of locale and DOM identity.
- Loading, resource-failure, unsafe-publication, and access-revoked states with English and Simplified Chinese copy.
- React-independent progress states: `idle`, `dirty`, `saving`, `synced`, `offline`, `conflict`, and `inaccessible`.
- Account-scoped persistence key and payload using authenticated `user_id`, manifestation ID, and install/device ID. Unscoped legacy records are ignored and removed.
- Server-side account binding on progress PUT, with private/no-store `403 progress_account_mismatch` before any progress command is applied.
- Debounced ordered writes, monotonically accepted versions, exact mutation/base retry, bounded lifecycle delivery, and online recovery that repeats a failed initial GET before replaying writes.
- Startup-read revision gating: a late GET cannot regress a newer accepted PUT/conflict/access-loss state.
- Conflict choices preserve and rebase a newer queued Locator for both global and device choices; choosing the device state still issues a fresh mutation against the authoritative version.
- Bounded persistence: no more than 32 pending positions and 64 KiB per account/manifestation/device record.
- Permission loss removes reader content while retaining the account-scoped retry record.

## GREEN evidence

Focused Task 21 unit/component suite:

```text
pnpm --dir web exec vitest run \
  src/features/reader/reader.test.tsx \
  src/features/reader/progress-sync.test.ts
EXIT 0
Test Files 2 passed (2)
Tests 21 passed (21)
```

The component suite was repeated three times after hardening its asynchronous layout assertion; all three runs passed 8/8 reader tests.

Complete Web suite and production checks:

```text
pnpm --dir web exec vitest run
EXIT 0
Test Files 6 passed (6)
Tests 67 passed (67)

pnpm --dir web lint
EXIT 0

pnpm --dir web typecheck
EXIT 0

pnpm --dir web build
EXIT 0
151 modules transformed
```

Real-browser coverage:

```text
pnpm --dir web exec playwright test
EXIT 0
4 passed (10.1s)
```

The Chromium suite verifies computed layout and scroll metrics, empty-sandbox parent isolation, Blob revocation, modal focus containment/background shortcut blocking, access loss, a stale conflict across two isolated browser contexts sharing one account, same-install account replacement, and initial-GET online recovery.

Changed HTTP contracts and routes:

```text
cargo test -p folioharbor-http \
  --test reader_routes --test auth_routes --test openapi_contract
EXIT 0
reader_routes: 14 passed
auth_routes: 12 passed
openapi_contract: 4 passed

cargo clippy -p folioharbor-http \
  --lib --test auth_routes --test reader_routes --test openapi_contract \
  --no-deps -- -D warnings -A clippy::struct_excessive_bools
EXIT 0

cargo fmt --all -- --check
EXIT 0
```

Generated-client and whitespace checks:

```text
pnpm --dir web generate-api
generated file SHA unchanged

git diff --check
EXIT 0
```

## Verification boundaries

- `cargo test --workspace` reaches the existing `folioharbor-api/tests/upload_composition.rs` database integration test and stops because PostgreSQL test configuration is absent. `cargo test -p folioharbor-http` similarly reaches an existing catalog database integration case after its unit/auth tests pass. No database-backed code changed in Task 21; all affected route and contract suites pass.
- Repository-wide pedantic Clippy is already blocked by pre-existing excessive-boolean capability DTOs and an unrelated `items_after_statements` finding. The changed HTTP library and three changed test targets pass Clippy with only the pre-existing capability-DTO lint allowed.
- jsdom continues to print its known canvas diagnostic while axe evaluates color contrast, plus one React 19 async test-environment diagnostic. The assertions pass, and the security/layout/focus behaviors now also run in real Chromium.
- During the account-replacement Playwright case, Chromium's unload keepalive bypasses Playwright routing and reaches the Vite proxy, which logs an expected connection refusal because no backend is running. The production defense for that exact race is covered at the real HTTP boundary: the mutation's `accountId` must equal the authenticated actor.
