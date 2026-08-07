# Task 21 Report: Secure EPUB Reader and Cross-Device Progress UX

## Status

Implemented the Task 21 reader vertical slice. The Web application now opens an Item's Readium-style publication manifest, validates every publication link against that Item's opaque authorized resource route, renders fetched publication bytes only through a privilege-free sandboxed Blob iframe, and exposes keyboard-accessible navigation and reading preferences. Reading progress is handled by a React-independent state machine with stable device identity, ordered offline retry, idempotent mutation reuse, bounded lifecycle delivery, explicit conflict choices, and fail-closed access-loss behavior.

No server or OpenAPI extension was necessary: the Task 14/15 manifest, resource, Locator, progress, and conflict contracts already covered this slice. The only shared client extension is an optional Fetch `keepalive` flag for bounded visibility/pagehide delivery.

## RED evidence

Reader tests were introduced before the route and reader modules:

- the first run failed because `locator.ts` did not exist;
- after the minimal Locator helper, five of six reader scenarios failed because the reader route, isolated frame, navigation, settings, resource states, and security validation did not exist;
- saved-progress and conflict-UI tests then failed until `ReaderPage` subscribed to the progress state machine;
- the conflict dialog autofocus assertion failed until the first explicit choice received focus.

Progress synchronization tests were also written before `ProgressSync.ts`:

- the first run failed because the state-machine module did not exist;
- after the initial implementation, the stable-device-ID check exposed an overly restrictive UUID validator;
- a lifecycle race regression first observed one ordinary request instead of the required bounded duplicate of the exact in-flight mutation, then passed after bounded lifecycle delivery reused that command and mutation ID.

The RED/GREEN cycles covered two devices sharing one fake account, including the deliberately smaller device position winning after an explicit user choice. No implementation or assertion resolves a conflict using maximum percentage.

## Delivered behavior

- Nested `/libraries/:libraryId/items/:itemId/read` route using the existing authenticated library shell.
- Typed manifest/progress DTOs from `web/src/api/generated.ts`; no parallel hand-written server schema.
- Same-origin, exact-Item resource allowlist with opaque resource IDs, no queries, and validation before any publication resource fetch.
- Sanitized bytes fetched with credentials and loaded through revocable Blob URLs.
- `<iframe sandbox="">` with no scripts, forms, popups, top-navigation, or same-origin privilege; publication HTML is never inserted into the parent DOM.
- Opaque-link TOC and previous/next navigation, arrow-key navigation, Escape close, initial modal focus, and focus return.
- Reduced-motion behavior plus persisted font scale and paginated/continuous-flow preferences.
- Readium Locator reporting independent of locale and DOM selectors/identity.
- Loading, resource failure, unsafe-publication, and access-revoked states with English and Simplified Chinese copy.
- UI-independent `ProgressSync` states: `idle`, `dirty`, `saving`, `synced`, `offline`, `conflict`, and `inaccessible`.
- Debounced writes, monotonically accepted versions, exact mutation/base-version retry, ordered offline replay, and online retry.
- Visibility/pagehide bounded flush. If an ordinary write is already in flight, one bounded delivery reuses the exact command and mutation ID so the server's idempotency contract remains authoritative.
- Stable, resettable per-install `device_id` stored only as client synchronization metadata and never treated as authentication.
- Bounded persistence: at most 32 pending positions and at most 64 KiB per manifestation/device record; only device ID, version, and safe-retry mutation data are stored.
- Explicit account/device conflict dialog showing both percentages. Choosing the account position performs no write; choosing the device position issues a new mutation against the current global version, even when its percentage is smaller.
- Permission loss transitions to `inaccessible`, removes reader content, and preserves the pending retry record rather than silently dropping local progress.

## GREEN evidence

Focused Task 21 suite:

```text
pnpm --dir web exec vitest run \
  src/features/reader/reader.test.tsx \
  src/features/reader/progress-sync.test.ts
EXIT 0
Test Files 2 passed (2)
Tests 16 passed (16)
```

All Web tests, run in isolation after an intentionally parallel gate run caused unrelated short UI timeouts:

```text
pnpm --dir web test -- --run
EXIT 0
Test Files 6 passed (6)
Tests 62 passed (62)
```

Static and production checks:

```text
pnpm --dir web lint
EXIT 0

pnpm --dir web typecheck
EXIT 0

pnpm --dir web build
EXIT 0
149 modules transformed

git diff --check
EXIT 0
```

Generated-client stability:

```text
pnpm --dir web generate-api
git diff --exit-code -- web/src/api/generated.ts
EXIT 0
```

Accessibility and security assertions in the reader component suite report zero axe violations and verify the empty iframe sandbox, prohibited sandbox privileges, Blob source, opaque authorized resource requests, parent-DOM isolation, modal focus, Escape handling, and focus return.

## Browser-harness boundary

The repository does not currently install Playwright or another multi-context browser-test dependency (`pnpm exec playwright --version` is unavailable). Per Task 23's ownership of the Playwright accessibility/security harness, Task 21 did not add a one-off browser dependency or duplicate that infrastructure. The shared-account/two-device protocol behavior is covered now by the production `ProgressSync` state machine against one shared fake API; the real two-isolated-browser-context scenario remains for the Task 23 harness.

## Concerns and follow-up

- axe-core passes its asserted zero-violation scan, but jsdom prints its existing `HTMLCanvasElement.getContext` not-implemented diagnostic while evaluating color contrast. A real-browser axe pass in Task 23 will remove that environment limitation.
- React 19 prints a non-failing test-environment `act`/Suspense diagnostic while the native visibility event triggers an asynchronous conflict response. The test awaits the resulting dialog and passes; production behavior is covered by both the component assertion and the UI-independent bounded-flush tests.
- The sandbox deliberately omits `allow-same-origin`. Reading-flow and font preferences therefore operate at the frame shell instead of mutating publication DOM, preserving the stronger isolation boundary.
