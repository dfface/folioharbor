# Task 20 Report: Library, Catalog, Upload, and Collaboration UX

## Status

Implemented the Task 20 Web vertical slice and the smallest server/OpenAPI additions required to make it secure and usable. Library identity remains route-derived, effective capabilities remain server-derived, invitation outcomes expose only safe state, and terminal uploads can link to the server-selected Item.

## RED evidence

The three feature suites were written before their pages and routes. The first focused run failed because the library switcher, nested library routes, upload form/status UI, member management, settings, and invitation acceptance page did not exist.

Backend contract tests were also introduced before implementation:

- the OpenAPI library test failed because `/api/v1/invitations/accept` was absent;
- the upload contract test failed because `UploadStatus` had no `item_id`;
- PostgreSQL-backed route tests established the required server-authoritative role matrix and safe invitation state behavior.

## Delivered behavior

- Route-scoped personal/shared library switcher with no instance-global all-books route.
- Server-capability-driven owner/editor/reader navigation and direct-route guards, without treating hidden controls as authorization.
- Library catalog with opaque keyset pagination, reloadable Item detail links, user-facing Work / Edition / Library copy concepts, and independent read/download actions.
- XHR isolated to the upload adapter for transmitted-byte progress, 1 GiB precheck, cancel, durable status polling, all eleven lifecycle states, and Ready/Duplicate Item links.
- Owner member invitation, role change, removal, final-owner server-error handling, and reader-download settings.
- Logged-out invitation return links and authenticated accepted/wrong-account/unverified/expired/consumed/invalid states; wrong-account email is masked by the application layer.
- English and Simplified Chinese copy plus axe-core accessibility assertions on representative and lifecycle pages.
- OpenAPI-generated Web DTOs for library capabilities, member operations, invitation acceptance, and upload Item targets.
- Migration `0026_web_contracts.sql` for server-derived visible-library capabilities, detailed invitation acceptance, and atomic upload result Item persistence.

## GREEN evidence

Focused Web suite:

```text
pnpm --dir web test --run \
  src/features/libraries/library-flow.test.tsx \
  src/features/uploads/upload-flow.test.tsx \
  src/features/members/member-flow.test.tsx
EXIT 0
Test Files 3 passed (3)
Tests 27 passed (27)
```

All Web component tests:

```text
pnpm --dir web test --run
EXIT 0
Test Files 4 passed (4)
Tests 40 passed (40)
```

Static and production checks:

```text
pnpm --dir web lint
EXIT 0

pnpm --dir web typecheck
EXIT 0

pnpm --dir web build
EXIT 0
141 modules transformed

cargo fmt --check
EXIT 0

git diff --check
EXIT 0
```

Generated-client stability:

```text
shasum -a 256 web/src/api/generated.ts
pnpm --dir web generate-api
shasum -a 256 web/src/api/generated.ts
EXIT 0
Both hashes: e7953a1b312fe3160704acc98bad71cb5a583ef3f759a45e2131414f1a2ff390
```

Focused server contract/integration checks:

```text
cargo test -p folioharbor-http --test library_routes \
  openapi_exposes_the_complete_library_authorization_surface
EXIT 0 (1 passed)

cargo test -p folioharbor-http --test upload_routes \
  openapi_documents_upload_limits_states_media_and_retry_contract
EXIT 0 (1 passed)

FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres \
  cargo test -p folioharbor-http --test library_routes \
  invitation_acceptance_reports_safe_states_and_keeps_the_personal_library
EXIT 0 (1 passed)

FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres \
  cargo test -p folioharbor-http --test library_routes \
  concrete_routes_enforce_role_matrix_and_correlate_denial_audits
EXIT 0 (1 passed)

FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres \
  cargo test -p folioharbor-http --test upload_routes \
  duplicate_upload_status_exposes_the_existing_item_target
EXIT 0 (1 passed)
```

## Unfinished Task 20 items

No known functional Task 20 item is intentionally omitted from the implementation. The following broader verification was not completed before this reviewable checkpoint:

- no manual live-browser keyboard walkthrough against a running Rust server was performed; Testing Library `userEvent` exercises the interaction paths and axe-core reports zero violations in the asserted pages;
- direct-route reader/editor denial does not have a separate component test for every members/settings/upload permutation, although each page guards on server capabilities and the PostgreSQL-backed HTTP role-matrix test passes;
- the entire Rust workspace test suite was not rerun; only the new/affected HTTP/OpenAPI/PostgreSQL tests were run.

## Concerns and follow-up

- axe-core passes, but jsdom logs its expected `HTMLCanvasElement.getContext` not-implemented diagnostic while evaluating color contrast. This does not change the zero-violation assertions or test exit status; a real-browser accessibility pass remains valuable.
- The plan had reserved migration number `0026` for later Task 22 work. Task 22 must use the next available migration number (or be renumbered during integration).
- The Web currently links Item read actions to the Task 21 reader route; implementing that route remains Task 21, outside this slice.
