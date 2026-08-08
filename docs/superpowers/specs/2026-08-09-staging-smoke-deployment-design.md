# Staging deployment smoke-test design

## Purpose

Provide a repeatable operator entry point for deploying the first EPUB vertical
slice into a staging environment and performing manual smoke acceptance.  This
is a real deployment using the production-shaped Compose topology; it does not
run Playwright or replace the completed automated E2E suite.

## Environment boundary

Staging is isolated from production while using the same `deploy/compose.yaml`:

- `deploy/.env.staging` holds staging's non-secret deployment configuration and
  is never committed.
- `deploy/secrets.staging/` holds staging's eight required secret files and is
  never committed.
- `deploy/staging.env.example` is a committed, non-secret starting template.
  It documents the image, public HTTPS URL, loopback bind address, SMTP,
  sender, capacity limits, worker concurrency, and optional OTLP endpoint.
- `FOLIOHARBOR_COMPOSE_PROJECT` in the staging environment is
  `folioharbor-staging` by default.  Its containers, network, and named volumes
  are therefore distinct from production.

The staging environment file overrides every `*_FILE` variable so Compose reads
only `deploy/secrets.staging/`; it must not fall back to `deploy/secrets/`.
The repository ignore rules cover both staging paths.

## Operator interface

`scripts/smoke.sh` accepts an optional `--env-file PATH`, defaulting to
`deploy/.env.staging`, and exposes these commands:

- `check` validates prerequisite tools, the environment file, all eight secret
  file paths, restrictive permissions, non-empty values, and rendered Compose
  configuration.  It does not start containers.
- `up [--admin-email EMAIL]` runs `check`, starts PostgreSQL and storage
  initialization, runs migrations, interactively creates (or safely reuses) an
  administrator, starts API, Worker, and Web, then waits for their health
  checks.  It prints the public URL and the manual smoke checklist.
- `status` prints the Compose service state for this environment.
- `logs [SERVICE]` follows all staging services or one named service.
- `down` stops the staging services without deleting data volumes.
- `destroy` is the only destructive command.  It requires a literal
  confirmation of the configured project name before running Compose teardown
  with volumes removed.
- `smoke` prints the checklist without changing deployment state.

The script never creates credentials, changes deployment configuration, or
silently falls back to development services.  Missing or malformed deployment
configuration is an actionable failure.

## Deployment flow and failures

The script invokes Compose from `deploy/`, with the requested environment file,
so relative secret paths and PostgreSQL initialization files retain their
documented meaning.  It preserves Compose's dependency order:

1. validate configuration;
2. start `postgres` and `storage-init` and wait for success/readiness;
3. run the one-shot `migration` service;
4. create the administrator through its interactive CLI;
5. start `api`, `worker`, and `web`; and
6. wait for `api` and `web` to be healthy.

On a failure, the script leaves containers and data intact for `status` and
`logs`; it reports the failed phase and the exact follow-up command.  It never
uses `down -v` except after explicit `destroy` confirmation.

## Manual smoke checklist

After a successful `up`, the operator verifies against the configured staging
public URL:

1. Sign in with the administrator created by the bootstrap step.
2. Upload a representative EPUB and wait for it to appear in the library.
3. Open the book in the reader and navigate between chapters.
4. Refresh the reader, reopen the book, and confirm reading progress persists.
5. Return to the library and confirm the book and progress remain visible.
6. Check `scripts/smoke.sh status` and review service logs if any step fails.

SMTP delivery, account verification, and automated browser assertions are not
part of this manual smoke script; the operator configures real SMTP when those
deployment behaviours are in scope, and the existing E2E suite remains their
automated coverage.

## Verification

Shell-level contract tests will first demonstrate that a missing staging
configuration or secret is rejected, that `check` renders the requested
environment rather than production defaults, and that `smoke` is
non-destructive.  The implementation will then be verified with the contract
tests, shell syntax checking, and `docker compose config --quiet` using a
temporary non-secret fixture.  A real staging `up` is deliberately an operator
action because it uses operator-supplied credentials and external services.
