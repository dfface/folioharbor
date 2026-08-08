# Staging Smoke Deployment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide a safe, repeatable staging deployment launcher and manual EPUB smoke checklist that uses the production-shaped Docker Compose topology without touching production configuration or data.

**Architecture:** A Bash operator script resolves the repository and Compose paths once, reads an explicitly selected staging environment file, and delegates lifecycle work to `docker compose`. A committed non-secret staging template documents all deployment inputs; ignored staging environment and secret paths isolate the real configuration. A shell contract test replaces `docker` with a fixture executable so safety and command construction are verified without starting containers.

**Tech Stack:** Bash 3.2+ (macOS compatible), Docker Compose v2, POSIX filesystem permissions, existing `deploy/compose.yaml` and `deploy/example.env`.

**Source specification:** [`../specs/2026-08-09-staging-smoke-deployment-design.md`](../specs/2026-08-09-staging-smoke-deployment-design.md)

## Global Constraints

- The default staging paths are `deploy/.env.staging` and `deploy/secrets.staging/`; both must be ignored by Git and neither may be created, overwritten, or read from production paths by the script.
- Staging is production-shaped: use `deploy/compose.yaml`, an operator-selected image, real SMTP settings, and a public HTTPS URL; do not inject Mailpit or synthetic credentials.
- The script must run on Bash 3.2 and use `set -euo pipefail`; do not use associative arrays, `mapfile`, or Bash 4-only syntax.
- `check` performs no Docker lifecycle mutation; `smoke` performs no Docker invocation; `down` never removes volumes; only `destroy` may remove volumes after literal project-name confirmation.
- Secret inputs are the existing eight Compose secret files. All must be non-empty regular files, mode `0600` or stricter, in the configured staging secret directory; the directory must be mode `0700` or stricter.
- The script must reject a configuration that points to `deploy/secrets/`, and must require `FOLIOHARBOR_COMPOSE_PROJECT` to be set so staging resources cannot accidentally use the default project name.
- Every behavior change begins with a failing focused test, then the smallest implementation, then focused and broader verification, then a commit.

## Planned Repository Shape

```text
.
├── .gitignore                              # ignores staging operator inputs
├── deploy/
│   ├── README.md                            # staging configuration and operator guide
│   ├── compose.yaml                         # reused unchanged
│   └── staging.env.example                  # committed non-secret staging template
└── scripts/
    ├── smoke.sh                             # staging lifecycle and checklist CLI
    └── smoke.test.sh                        # Docker-free shell contract tests
```

## Stable Script Interface

```text
scripts/smoke.sh [--env-file PATH] <command> [arguments]

commands:
  check
  up --admin-email EMAIL
  status
  logs [SERVICE]
  down
  destroy
  smoke
```

`--env-file` defaults to `deploy/.env.staging`, is interpreted from the
repository root, and is passed unchanged to Compose. The configuration must
define `FOLIOHARBOR_COMPOSE_PROJECT`, `FOLIOHARBOR_IMAGE`,
`FOLIOHARBOR_PUBLIC_BASE_URL`, `FOLIOHARBOR_HTTP_BIND`, SMTP and sender values,
and the eight `FOLIOHARBOR_*_FILE` secret paths.

---

### Task 1: Define the isolated staging configuration contract

**Files:**
- Modify: `.gitignore`
- Create: `deploy/staging.env.example`
- Modify: `deploy/README.md`
- Test: `scripts/smoke.test.sh`

**Interfaces:**
- Produces the exact ignored operator paths `deploy/.env.staging` and `deploy/secrets.staging/`.
- Produces `deploy/staging.env.example`, which `scripts/smoke.sh --env-file deploy/.env.staging check` consumes in Task 2 after it has been copied and populated.

- [ ] **Step 1: Write failing configuration-template contract tests**

Create `scripts/smoke.test.sh` with a portable test runner that records failures and exits non-zero. Copy `deploy/staging.env.example` to a temporary fixture, source that fixture in a child shell, and assert its observable configuration contract rather than grepping source text. The fixture must yield a staging project, isolated secret paths, and real-deployment endpoints; also assert that Git ignores the two real staging paths:

```bash
assert_equals folioharbor-staging "$(read_fixture_value FOLIOHARBOR_COMPOSE_PROJECT)"
assert_equals ./secrets.staging/postgres-password \
  "$(read_fixture_value FOLIOHARBOR_POSTGRES_PASSWORD_FILE)"
assert_equals ./secrets.staging/application-secret \
  "$(read_fixture_value FOLIOHARBOR_APPLICATION_SECRET_FILE)"
assert_equals https://staging-library.example/ \
  "$(read_fixture_value FOLIOHARBOR_PUBLIC_BASE_URL)"
git check-ignore -q deploy/.env.staging
git check-ignore -q deploy/secrets.staging/application-secret
```

Run: `bash scripts/smoke.test.sh`

Expected: FAIL because the template and ignore rules do not yet exist.

- [ ] **Step 2: Add the minimum isolated configuration contract**

Append these exact ignore entries to `.gitignore`:

```gitignore
deploy/.env.staging
deploy/secrets.staging/
```

Create `deploy/staging.env.example` by copying the non-secret settings from
`deploy/example.env`, then set these staging-specific values and include all
eight file variables:

```dotenv
FOLIOHARBOR_COMPOSE_PROJECT=folioharbor-staging
FOLIOHARBOR_IMAGE=ghcr.io/folioharbor/folioharbor:0.1.0
FOLIOHARBOR_PUBLIC_BASE_URL=https://staging-library.example/
FOLIOHARBOR_HTTP_BIND=127.0.0.1:18080
FOLIOHARBOR_MAIL_SMTP_URL=smtps://smtp.example:465
FOLIOHARBOR_MAIL_FROM_ADDRESS=noreply@staging-library.example
FOLIOHARBOR_POSTGRES_PASSWORD_FILE=./secrets.staging/postgres-password
FOLIOHARBOR_OWNER_PASSWORD_FILE=./secrets.staging/owner-password
FOLIOHARBOR_API_PASSWORD_FILE=./secrets.staging/api-password
FOLIOHARBOR_WORKER_PASSWORD_FILE=./secrets.staging/worker-password
FOLIOHARBOR_OWNER_DATABASE_URL_FILE=./secrets.staging/owner-database-url
FOLIOHARBOR_API_DATABASE_URL_FILE=./secrets.staging/api-database-url
FOLIOHARBOR_WORKER_DATABASE_URL_FILE=./secrets.staging/worker-database-url
FOLIOHARBOR_APPLICATION_SECRET_FILE=./secrets.staging/application-secret
```

Keep the quota, retention, worker-concurrency, logging, key-ID, UID/GID, and
optional OTLP settings visible with explanatory comments. Do not put secret
values, SMTP credentials, or a production hostname in the file.

- [ ] **Step 3: Document exact operator preparation**

Add a `## Staging manual acceptance` section to `deploy/README.md`. It must
instruct the operator to copy `staging.env.example` to `.env.staging`, create
`secrets.staging` with mode `0700`, create each named secret with mode `0600`,
use independent role passwords, and build URL-encoded role-specific PostgreSQL
URLs using the Compose host `postgres`. Include the precise preparation and
preflight commands:

```sh
cp deploy/staging.env.example deploy/.env.staging
mkdir -m 0700 deploy/secrets.staging
chmod 600 deploy/secrets.staging/*
scripts/smoke.sh check
```

State that `FOLIOHARBOR_PUBLIC_BASE_URL` must be the HTTPS reverse-proxy URL,
`FOLIOHARBOR_HTTP_BIND` must remain loopback-bound, and staging SMTP must be a
real staging SMTP service.

- [ ] **Step 4: Verify the configuration contract passes**

Run: `bash scripts/smoke.test.sh`

Expected: PASS for template and ignore assertions; lifecycle assertions may be
added in later tasks but must not be run until their tests exist.

- [ ] **Step 5: Commit the isolated configuration contract**

```bash
git add .gitignore deploy/staging.env.example deploy/README.md scripts/smoke.test.sh
git commit -m "docs: add staging deployment configuration"
```

### Task 2: Implement non-mutating preflight and smoke checklist

**Files:**
- Create: `scripts/smoke.sh`
- Modify: `scripts/smoke.test.sh`
- Modify: `deploy/README.md`

**Interfaces:**
- Consumes `deploy/.env.staging` and the eight `FOLIOHARBOR_*_FILE` variables from Task 1.
- Produces `scripts/smoke.sh [--env-file PATH] check` and `scripts/smoke.sh smoke` for Task 3.

- [ ] **Step 1: Add failing script contract tests**

Extend `scripts/smoke.test.sh` to create a temporary copy of the template and a
`secrets.staging` fixture with all eight non-empty files at mode `0600`. Put a
fake `docker` executable first in `PATH`; it appends arguments to
`$SMOKE_TEST_DOCKER_LOG` and exits zero. Test these observable behaviours:

```bash
run_smoke --env-file "$fixture_env" check
assert_file_contains "$SMOKE_TEST_DOCKER_LOG" \
  "compose --env-file $fixture_env --project-name folioharbor-staging -f $repo/deploy/compose.yaml config --quiet"

rm "$fixture_secrets/api-password"
assert_command_fails run_smoke --env-file "$fixture_env" check
assert_output_contains 'missing or empty secret file'

run_smoke smoke
assert_file_empty "$SMOKE_TEST_DOCKER_LOG"
assert_output_contains 'Upload a representative EPUB'
```

Run: `bash scripts/smoke.test.sh`

Expected: FAIL because `scripts/smoke.sh` does not exist.

- [ ] **Step 2: Implement parsing, validation, and `smoke`**

Create executable `scripts/smoke.sh` using `#!/usr/bin/env bash` and
`set -euo pipefail`. Resolve `repository_root` from `BASH_SOURCE`, default the
environment path to `$repository_root/deploy/.env.staging`, and reject paths
outside the repository only when they are relative paths containing `..`.
Load only safe dotenv assignment lines with `set -a; . "$env_file"; set +a`;
the template must remain shell-compatible.

Implement `check` to require `docker`, `FOLIOHARBOR_COMPOSE_PROJECT`, the eight
file-variable names, a secret-directory path containing `secrets.staging`,
non-empty regular secret files, and numeric permission modes no broader than
`0700` for the directory and `0600` for files. End with exactly this Compose
preflight shape:

```bash
docker compose --env-file "$env_file" \
  --project-name "$FOLIOHARBOR_COMPOSE_PROJECT" \
  -f "$repository_root/deploy/compose.yaml" config --quiet
```

Implement `smoke` as text only. Print the configured
`FOLIOHARBOR_PUBLIC_BASE_URL` if the selected environment file exists, then the
six checklist items from the design. It must not call Docker or source an
environment file that does not exist.

- [ ] **Step 3: Verify focused contracts**

Run: `bash scripts/smoke.test.sh`

Expected: PASS, including missing-secret rejection, Compose command rendering,
and Docker-free `smoke` output.

- [ ] **Step 4: Document safe preflight and checklist usage**

In the staging README section, document:

```sh
scripts/smoke.sh check
scripts/smoke.sh smoke
```

Explain that `check` validates but does not start containers, while `smoke`
only prints the manual checklist.

- [ ] **Step 5: Commit the preflight CLI**

```bash
git add scripts/smoke.sh scripts/smoke.test.sh deploy/README.md
git commit -m "feat: add staging smoke preflight"
```

### Task 3: Add safe Compose lifecycle commands and operator documentation

**Files:**
- Modify: `scripts/smoke.sh`
- Modify: `scripts/smoke.test.sh`
- Modify: `deploy/README.md`

**Interfaces:**
- Consumes the validated Compose invocation and `check` command from Task 2.
- Produces the complete operator interface: `up --admin-email`, `status`,
  `logs [SERVICE]`, `down`, and confirmation-gated `destroy`.

- [ ] **Step 1: Add failing lifecycle contract tests**

Extend the fake-Docker tests to assert exact command ordering and destructive
boundaries:

```bash
run_smoke --env-file "$fixture_env" up --admin-email admin@staging.example
assert_log_in_order "$SMOKE_TEST_DOCKER_LOG" \
  'config --quiet' \
  'up -d postgres storage-init migration' \
  'run --rm migration folioharbor admin create --email admin@staging.example' \
  'up -d api worker web'

run_smoke --env-file "$fixture_env" down
assert_file_contains "$SMOKE_TEST_DOCKER_LOG" 'down --remove-orphans'
assert_file_not_contains "$SMOKE_TEST_DOCKER_LOG" 'down -v'

printf 'wrong-project\n' | assert_command_fails run_smoke --env-file "$fixture_env" destroy
assert_file_not_contains "$SMOKE_TEST_DOCKER_LOG" 'down -v --remove-orphans'

printf 'folioharbor-staging\n' | run_smoke --env-file "$fixture_env" destroy
assert_file_contains "$SMOKE_TEST_DOCKER_LOG" 'down -v --remove-orphans'
```

Run: `bash scripts/smoke.test.sh`

Expected: FAIL because lifecycle commands are not implemented.

- [ ] **Step 2: Implement lifecycle commands with phase-specific failures**

Make `up` require one syntactically non-empty `--admin-email` argument; invoke
`check`, then run these commands from the repository root with the shared
Compose argument list:

```bash
docker compose ... up -d postgres storage-init migration
docker compose ... run --rm migration folioharbor admin create --email "$email"
docker compose ... up -d --wait api worker web
docker compose ... ps
```

The admin-create command keeps stdin attached so its password prompt remains
interactive. On any failure, print the phase and
`scripts/smoke.sh --env-file <path> logs`; do not call `down`.

Implement `status` as `docker compose ... ps`, `logs [SERVICE]` as
`docker compose ... logs --follow --tail 200 [SERVICE]`, and `down` as
`docker compose ... down --remove-orphans`. Reject a `SERVICE` argument that
starts with `-`. Implement `destroy` by printing the project name and reading
one line from the terminal; only exact equality permits
`docker compose ... down -v --remove-orphans`.

- [ ] **Step 3: Verify lifecycle contracts and shell syntax**

Run:

```bash
bash -n scripts/smoke.sh scripts/smoke.test.sh
bash scripts/smoke.test.sh
```

Expected: both commands exit 0. The fake Docker log must prove that `down`
does not include `-v`, `destroy` requires confirmation, and `up` preserves the
documented dependency order.

- [ ] **Step 4: Complete the staging operator guide**

Document the exact lifecycle commands:

```sh
scripts/smoke.sh up --admin-email admin@staging.example
scripts/smoke.sh status
scripts/smoke.sh logs api
scripts/smoke.sh down
scripts/smoke.sh destroy
```

State that `destroy` removes staging PostgreSQL and Blob data, must only be
used for a disposable staging environment, and requires typing the configured
project name. Include the six manual EPUB smoke steps verbatim and explain how
to pass a non-default isolated configuration with
`--env-file deploy/.env.staging`.

- [ ] **Step 5: Verify against a disposable rendered staging fixture and commit**

Run:

```bash
bash -n scripts/smoke.sh scripts/smoke.test.sh
bash scripts/smoke.test.sh
git diff --check
```

Then create a temporary, non-secret fixture before the real Compose render so
the eight Compose secret-file references resolve without using either staging
or production inputs:

```bash
fixture_directory="$(mktemp -d)"
trap 'rm -rf "$fixture_directory"' EXIT
for secret in postgres-password owner-password api-password worker-password \
  owner-database-url api-database-url worker-database-url application-secret; do
  printf 'fixture-only-value' > "$fixture_directory/$secret"
  chmod 600 "$fixture_directory/$secret"
done
awk -v secret_directory="$fixture_directory" \
  '{ gsub("\\./secrets\\.staging", secret_directory); print }' \
  deploy/staging.env.example > "$fixture_directory/staging.env"
docker compose --env-file "$fixture_directory/staging.env" \
  -f deploy/compose.yaml config --quiet
```

Expected: all commands exit 0. The final Compose command renders configuration
only; it does not start containers and uses only temporary fixture files.

Commit:

```bash
git add scripts/smoke.sh scripts/smoke.test.sh deploy/README.md
git commit -m "feat: add staging smoke deployment workflow"
```
