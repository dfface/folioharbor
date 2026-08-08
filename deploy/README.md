# Official Compose deployment

`compose.yaml` is the supported single-host topology: PostgreSQL 18, one-shot storage initialization, one-shot schema migration, API, Worker, the production Web SPA, one shared storage volume (with managed `objects/` and `staging/` subdirectories), and an optional Mailpit development profile. The Web service serves fingerprinted assets with immutable caching, falls back to `index.html` for application deep links, applies browser security headers, and proxies same-origin `/api/` and `/health/` requests to the API. It intentionally does not bundle HTTPS termination.

## First start

Copy `example.env` to `.env`, create `deploy/secrets/` with mode `0700`, and create each file named in `.env` with mode `0600`. Use independent random values for the PostgreSQL administrator, owner, API, Worker, and application secrets. The three database-URL files must use the matching role password and the Compose hostname, for example `postgres://folioharbor_api:<url-encoded-password>@postgres/folioharbor`. No example file contains a usable credential.

Validate and start the topology:

```sh
docker compose --env-file .env -f compose.yaml config --quiet
docker compose --env-file .env -f compose.yaml up -d postgres storage-init migration
docker compose --env-file .env -f compose.yaml run --rm migration \
  folioharbor admin create --email admin@example.com
docker compose --env-file .env -f compose.yaml up -d api worker web
```

The bootstrap command requires an interactive TTY and prompts twice; it rejects password arguments and administrator-password environment variables. Re-running it for the same administrator is safe. Public registration remains `503 bootstrap_required` until this succeeds.

Use `--profile development` to run Mailpit. Production must set an external SMTP URL (and, when needed, inject mail credentials through environment or `_FILE`) before starting API and Worker.

## Security and lifecycle

API and Worker never receive the owner URL. Migration and explicit operator commands never reuse runtime credentials. The shared `blob_data` volume is initialized as UID/GID `10001:10001` with directories mode `0700`; set `FOLIOHARBOR_UID` and `FOLIOHARBOR_GID` to the non-root identity in a custom image. Both runtime services mount the same volume so Blob promotion and staging recovery see one filesystem.

The active limits are starting examples, not sizing guarantees. Worker concurrency defaults to `1`. API and Worker handle `SIGTERM`; Compose allows 30 seconds for graceful shutdown. PostgreSQL gets 60 seconds. Health checks distinguish process liveness from readiness, and migration must complete successfully before either runtime starts.

Terminate HTTPS at a separately managed reverse proxy in front of the loopback-bound Web port. Forward only trusted client metadata, preserve `traceparent`, and impose upload/time limits consistent with FolioHarbor. The API is internal to the Compose network; do not publish it, PostgreSQL, or the Blob volume directly.

Secrets may be supplied directly with the documented `FOLIOHARBOR_*` environment names or with the matching `_FILE` names. Prefer Compose secrets. Never commit `.env`, `deploy/secrets/`, database URLs, cookies, tokens, or SMTP passwords.

## Staging manual acceptance

Staging uses the same Compose topology as production, with separate configuration,
secrets, and Docker resources. Prepare its ignored inputs before starting it:

```sh
cp deploy/staging.env.example deploy/.env.staging
mkdir -m 0700 deploy/secrets.staging
# Create the eight secret files named by deploy/.env.staging, then:
chmod 600 deploy/secrets.staging/*
scripts/smoke.sh check
```

Set `FOLIOHARBOR_COMPOSE_PROJECT` to an isolated name (the template uses
`folioharbor-staging`). Set `FOLIOHARBOR_PUBLIC_BASE_URL` to the HTTPS URL of
the staging reverse proxy and keep `FOLIOHARBOR_HTTP_BIND` loopback-bound. Use
a real staging SMTP service and a staging sender address.

Create independent PostgreSQL administrator, owner, API, Worker, and application
secrets. The owner, API, and Worker database URLs must use the matching,
URL-encoded role password and the Compose hostname, for example
`postgres://folioharbor_api:<url-encoded-password>@postgres/folioharbor`.
The eight files are `postgres-password`, `owner-password`, `api-password`,
`worker-password`, `owner-database-url`, `api-database-url`,
`worker-database-url`, and `application-secret`.

Validate the configured staging topology before changing container state:

```sh
scripts/smoke.sh check
scripts/smoke.sh smoke
```

`check` validates the selected configuration, secret paths and permissions, and
the rendered Compose topology; it does not start containers. `smoke` only
prints the manual EPUB acceptance checklist and does not invoke Docker.

Start the staging deployment and create (or safely reuse) an administrator:

```sh
scripts/smoke.sh up --admin-email admin@staging.example
scripts/smoke.sh status
scripts/smoke.sh logs api
scripts/smoke.sh down
```

`up` starts PostgreSQL, storage initialization, and schema migration before it
prompts for the administrator password. It then starts API, Worker, and Web and
waits for their Compose health checks. If a phase fails, services and volumes
remain available for `status` and `logs` diagnosis.

`down` stops the isolated staging project and removes orphan containers, but
keeps the PostgreSQL and Blob volumes. For a disposable staging environment,
`scripts/smoke.sh destroy` permanently removes those volumes only after you
type the configured `FOLIOHARBOR_COMPOSE_PROJECT` exactly.

Use a different isolated environment file when needed:

```sh
scripts/smoke.sh --env-file deploy/.env.staging check
```

After a successful `up`, complete this manual EPUB smoke checklist:

1. Sign in with the staging administrator.
2. Upload a representative EPUB and wait for it to appear in the library.
3. Open the book in the reader and navigate between chapters.
4. Refresh the reader, reopen the book, and confirm reading progress persists.
5. Return to the library and confirm the book and progress remain visible.
6. Check `scripts/smoke.sh status` and review service logs if any step fails.
