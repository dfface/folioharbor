# End-to-end release journeys

The Playwright suite starts a clean, production-shaped Compose topology for every run:

- PostgreSQL 18 with the deployment role bootstrap and all migrations;
- the CLI administrator bootstrap as a one-shot service;
- separate least-privilege API and Worker processes;
- shared local Blob storage owned by the unprivileged runtime UID; and
- Mailpit for verification and invitation delivery.

The suite generates its own EPUB files, users, passwords, application secret, and database-role passwords. It does not use repository fixtures containing credentials or publication content.

## Run locally

Install the Web dependencies and Chromium once, then run from the repository root:

```bash
pnpm install --frozen-lockfile
pnpm --dir web exec playwright install chromium
pnpm --dir web exec playwright test
```

The runner builds `folioharbor-e2e-app:local`, removes any old `folioharbor-e2e` project and volumes, waits for the clean topology, and tears it down when Playwright exits. To reuse an already-built image while iterating:

```bash
FOLIOHARBOR_E2E_SKIP_BUILD=1 pnpm --dir web exec playwright test e2e/auth-library.spec.ts
```

Do not set the reuse flag unless `docker image inspect folioharbor-e2e-app:local` succeeds and the image contains the current source.

## Failure diagnosis

An image pull or build error is an infrastructure/registry failure. A container that starts but becomes unhealthy, an HTTP assertion failure, or a non-zero Worker exit is an application failure. Preserve that distinction in CI reports; never replace a failed pull, migration, readiness check, or user journey with a mocked success.

For local diagnosis, inspect live service status and logs before manually tearing down:

```bash
docker compose -p folioharbor-e2e -f tests/e2e/compose.test.yaml ps
docker compose -p folioharbor-e2e -f tests/e2e/compose.test.yaml logs api worker migration
docker compose -p folioharbor-e2e -f tests/e2e/compose.test.yaml down -v --remove-orphans
```

Logs and Playwright traces can contain identifiers and test-generated mail tokens. They are local-only diagnostics. CI uploads only the allowlisted service/state/health summary described in the workflow; it must not upload Compose logs, HTTP bodies, EPUB bytes, cookies, storage paths/keys, or traces.
