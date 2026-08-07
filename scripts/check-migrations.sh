#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
container_name="folioharbor-migration-check-$$"
secret_directory="$(mktemp -d)"

cleanup() {
  docker rm --force "$container_name" >/dev/null 2>&1 || true
  rm -rf "$secret_directory"
}

trap cleanup EXIT INT TERM

command -v docker >/dev/null 2>&1 || {
  echo "migration check requires Docker" >&2
  exit 1
}

command -v openssl >/dev/null 2>&1 || {
  echo "migration check requires openssl" >&2
  exit 1
}

umask 077
postgres_password="$(openssl rand -hex 32)"
export FOLIOHARBOR_TEST_OWNER_PASSWORD="$(openssl rand -hex 32)"
export FOLIOHARBOR_TEST_API_PASSWORD="$(openssl rand -hex 32)"
export FOLIOHARBOR_TEST_WORKER_PASSWORD="$(openssl rand -hex 32)"
printf '%s' "$postgres_password" > "$secret_directory/postgres-password"

docker run --detach --rm \
  --name "$container_name" \
  --publish 127.0.0.1::5432 \
  --env POSTGRES_DB=postgres \
  --env POSTGRES_USER=postgres \
  --env POSTGRES_PASSWORD_FILE=/run/secrets/postgres_password \
  --volume "$secret_directory/postgres-password:/run/secrets/postgres_password:ro" \
  postgres:18.0-alpine >/dev/null

for _ in $(seq 1 60); do
  # The official image briefly runs an init-only Unix-socket server before the
  # final TCP listener. Waiting on TCP avoids racing that intentional restart.
  if docker exec "$container_name" pg_isready --host 127.0.0.1 --username postgres --dbname postgres >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

docker exec "$container_name" pg_isready --host 127.0.0.1 --username postgres --dbname postgres >/dev/null
server_version="$(docker exec --env PGPASSWORD="$postgres_password" "$container_name" \
  psql --host 127.0.0.1 --username postgres --dbname postgres --tuples-only --no-align \
  --command 'SHOW server_version_num')"
if [[ "$server_version" != 180* ]]; then
  echo "migration check requires PostgreSQL 18, got server_version_num=$server_version" >&2
  exit 1
fi

host_port="$(docker port "$container_name" 5432/tcp | sed -E 's/^.*:([0-9]+)$/\1/' | head -n 1)"
if [[ ! "$host_port" =~ ^[0-9]+$ ]]; then
  echo "could not resolve the disposable PostgreSQL port" >&2
  exit 1
fi

export FOLIOHARBOR_TEST_DATABASE_URL="postgres://postgres:${postgres_password}@127.0.0.1:${host_port}/postgres"
cd "$repository_root"

cargo test --locked -p folioharbor-postgres --test migration_from_zero \
  committed_task_base_upgrades_without_sqlx_checksum_drift -- --exact
cargo test --locked -p folioharbor-postgres --test migration_from_zero \
  migrations_from_zero_preserve_least_privilege_roles_and_are_idempotent -- --exact
cargo test --locked -p folioharbor-postgres --test migration_from_zero \
  runtime_roles_reject_other_role_credentials -- --exact
cargo test --locked -p folioharbor-postgres --test rls_matrix
cargo test --locked -p folioharbor-postgres --test library_repository \
  personal_library_repository_retry_returns_the_same_library -- --exact
cargo test --locked -p folioharbor-postgres --test import_cleanup \
  received_expiry_releases_quota_and_honors_configured_failed_retention -- --exact
cargo test --locked -p folioharbor-postgres --test import_cleanup \
  failed_transition_and_purge_schedule_are_atomic_and_reconciliation_repairs_legacy_rows -- --exact
cargo test --locked -p folioharbor-postgres --test blob_gc \
  delete_restore_honors_configured_recovery_period_and_audit_is_atomic -- --exact
cargo test --locked -p folioharbor-postgres --test blob_gc \
  purge_releases_quota_removes_cache_derivatives_and_preserves_progress_and_audit -- --exact
cargo test --locked -p folioharbor-postgres --test identity_repository \
  session_listing_and_revocation_apply_the_authenticated_user_rls_context -- --exact
cargo test --locked -p folioharbor-api --test upload_composition \
  production_upload_composition_uses_postgres_and_local_blob_storage -- --exact
cargo test --locked -p folioharbor-worker --test import_recovery \
  restart_after_catalog_commit_reconciles_once_and_finishes_the_leased_job -- --exact
