# Official Compose deployment

`compose.yaml` is the supported single-host topology: PostgreSQL 18, one-shot storage initialization, one-shot schema migration, API, Worker, one shared storage volume (with managed `objects/` and `staging/` subdirectories), and an optional Mailpit development profile. It intentionally does not bundle an HTTPS reverse proxy.

## First start

Copy `example.env` to `.env`, create `deploy/secrets/` with mode `0700`, and create each file named in `.env` with mode `0600`. Use independent random values for the PostgreSQL administrator, owner, API, Worker, and application secrets. The three database-URL files must use the matching role password and the Compose hostname, for example `postgres://folioharbor_api:<url-encoded-password>@postgres/folioharbor`. No example file contains a usable credential.

Validate and start the topology:

```sh
docker compose --env-file .env -f compose.yaml config --quiet
docker compose --env-file .env -f compose.yaml up -d postgres storage-init migration
docker compose --env-file .env -f compose.yaml run --rm migration \
  folioharbor admin create --email admin@example.com
docker compose --env-file .env -f compose.yaml up -d api worker
```

The bootstrap command requires an interactive TTY and prompts twice; it rejects password arguments and administrator-password environment variables. Re-running it for the same administrator is safe. Public registration remains `503 bootstrap_required` until this succeeds.

Use `--profile development` to run Mailpit. Production must set an external SMTP URL (and, when needed, inject mail credentials through environment or `_FILE`) before starting API and Worker.

## Security and lifecycle

API and Worker never receive the owner URL. Migration and explicit operator commands never reuse runtime credentials. The shared `blob_data` volume is initialized as UID/GID `10001:10001` with directories mode `0700`; set `FOLIOHARBOR_UID` and `FOLIOHARBOR_GID` to the non-root identity in a custom image. Both runtime services mount the same volume so Blob promotion and staging recovery see one filesystem.

The active limits are starting examples, not sizing guarantees. Worker concurrency defaults to `1`. API and Worker handle `SIGTERM`; Compose allows 30 seconds for graceful shutdown. PostgreSQL gets 60 seconds. Health checks distinguish process liveness from readiness, and migration must complete successfully before either runtime starts.

Terminate HTTPS at a separately managed reverse proxy. Forward only trusted client metadata, preserve `traceparent`, impose upload/time limits consistent with FolioHarbor, and proxy to the loopback-bound API port. Do not publish PostgreSQL or the Blob volume.

Secrets may be supplied directly with the documented `FOLIOHARBOR_*` environment names or with the matching `_FILE` names. Prefer Compose secrets. Never commit `.env`, `deploy/secrets/`, database URLs, cookies, tokens, or SMTP passwords.
