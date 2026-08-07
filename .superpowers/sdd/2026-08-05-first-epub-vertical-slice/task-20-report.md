# Task 20 Report: Library, Catalog, Upload, and Collaboration UX

## Status

Implemented the Task 20 Web vertical slice and the smallest server/OpenAPI additions required to make it secure and usable. Independent-review P1/P2 findings are repaired: authenticated resource caches are isolated across identities, route detail cannot reintroduce a library absent from the visible list, migration history is gap-free through 0026 without renaming durable history, invitation UI follows its dedicated capability and operation-specific role contract, and successful invitation navigation cannot be bounced by a cached pre-acceptance library list.

## RED evidence

The three feature suites were written before their pages and routes. The first focused run failed because the library switcher, nested library routes, upload form/status UI, member management, settings, and invitation acceptance page did not exist.

Backend contract tests were also introduced before implementation:

- the OpenAPI library test failed because `/api/v1/invitations/accept` was absent;
- the upload contract test failed because `UploadStatus` had no `item_id`;
- PostgreSQL-backed route tests established the required server-authoritative role matrix and safe invitation state behavior.

Independent-review repairs followed focused RED/GREEN cycles:

- the two-account Web regression initially rendered account A's library, member, capability links, catalog Item, and upload state while account B's resource responses were held pending; the unmatched-detail regression also stayed on account A's route;
- the invitation regressions initially rendered the form when `can_invite_members` was false and exposed role values `[reader, editor, owner]`; mutation checks also proved invite-only navigation, direct-route access, and suppression of member-list reads failed under the old gates;
- the migration-from-zero gate initially observed applied versions 1 through 26 while its invariant expected only 1 through 25.
- the final re-review race regression first cached the personal-library list, accepted an invitation in the same SPA, held the post-acceptance list response pending, and observed navigation bounce from the accepted-library route to `/`.

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
- One authenticated-resource query-key root covers library detail/list, catalog, member, and upload caches; every successful identity transition uses the Task 19 reset/remove flow for that complete subtree.
- Invitation and member-management controls are independently gated by `can_invite_members` and `can_manage_members`; invitation roles come directly from the generated operation request type and contain only `editor | reader`.
- Successful invitation acceptance removes only the exact cached library-list entry before navigation. The accepted route therefore waits for a fresh authoritative list, while library detail/catalog/member/upload caches and fail-closed authorization behavior remain intact.

## GREEN evidence

Focused Web suite:

```text
pnpm --dir web test --run \
  src/features/libraries/library-flow.test.tsx \
  src/features/uploads/upload-flow.test.tsx \
  src/features/members/member-flow.test.tsx
EXIT 0
Test Files 3 passed (3)
Tests 33 passed (33)
```

All Web component tests:

```text
pnpm --dir web test --run
EXIT 0
Test Files 4 passed (4)
Tests 46 passed (46)
```

Static and production checks:

```text
pnpm --dir web lint
EXIT 0

pnpm --dir web typecheck
EXIT 0

pnpm --dir web build
EXIT 0
142 modules transformed

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

Independent-review repair gates:

```text
FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres \
  cargo test -p folioharbor-postgres --test migration_from_zero \
  migrations_from_zero_preserve_least_privilege_roles_and_are_idempotent -- --exact
EXIT 0 (1 passed)

FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres \
  cargo test -p folioharbor-postgres --test library_repository \
  invitations_are_email_bound_expiring_single_use_and_preserve_personal_libraries -- --exact
EXIT 0 (1 passed)

FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres \
  cargo test -p folioharbor-postgres --test catalog_constraints \
  exact_blob_is_idempotent_per_library_but_shared_across_libraries -- --exact
EXIT 0 (1 passed)

FOLIOHARBOR_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55432/postgres \
  cargo test -p folioharbor-postgres --test catalog_constraints \
  concurrent_same_library_finalization_creates_one_active_item -- --exact
EXIT 0 (1 passed)
```

Migration sequence decision: preserve committed `0026_web_contracts.sql` and its checksum. The
governing plan now assigns the not-yet-implemented Task 22 operations migration to
`0027_operations.sql`; renaming 0026 after review or possible application would make durable SQLx
history unsafe.

## Unfinished Task 20 items

No known functional Task 20 item is intentionally omitted from the implementation. The following broader verification was not completed before this reviewable checkpoint:

- no manual live-browser keyboard walkthrough against a running Rust server was performed; Testing Library `userEvent` exercises the interaction paths and axe-core reports zero violations in the asserted pages;
- direct-route reader/editor denial does not have a separate component test for every members/settings/upload permutation, although each page guards on server capabilities and the PostgreSQL-backed HTTP role-matrix test passes;
- the entire Rust workspace test suite was not rerun; only the new/affected HTTP/OpenAPI/PostgreSQL tests were run.

## Concerns and follow-up

- axe-core passes, but jsdom logs its expected `HTMLCanvasElement.getContext` not-implemented diagnostic while evaluating color contrast. This does not change the zero-violation assertions or test exit status; a real-browser accessibility pass remains valuable.
- Task 22 must follow the amended governing-plan reservation and create `0027_operations.sql`; `0026_web_contracts.sql` is now immutable migration history.
- The Web currently links Item read actions to the Task 21 reader route; implementing that route remains Task 21, outside this slice.
