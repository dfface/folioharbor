#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
container_name="folioharbor-migration-check-$$"

cleanup() {
  docker rm --force "$container_name" >/dev/null 2>&1 || true
}

trap cleanup EXIT INT TERM

command -v docker >/dev/null 2>&1 || {
  echo "migration check requires Docker" >&2
  exit 1
}

docker run --detach --rm \
  --name "$container_name" \
  --publish 127.0.0.1::5432 \
  --env POSTGRES_DB=postgres \
  --env POSTGRES_USER=postgres \
  --env POSTGRES_HOST_AUTH_METHOD=trust \
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
server_version="$(docker exec "$container_name" psql --username postgres --dbname postgres --tuples-only --no-align --command 'SHOW server_version_num')"
if [[ "$server_version" != 180* ]]; then
  echo "migration check requires PostgreSQL 18, got server_version_num=$server_version" >&2
  exit 1
fi

host_port="$(docker port "$container_name" 5432/tcp | sed -E 's/^.*:([0-9]+)$/\1/' | head -n 1)"
if [[ ! "$host_port" =~ ^[0-9]+$ ]]; then
  echo "could not resolve the disposable PostgreSQL port" >&2
  exit 1
fi

export FOLIOHARBOR_TEST_DATABASE_URL="postgres://postgres@127.0.0.1:${host_port}/postgres"
cd "$repository_root"

cargo test --locked -p folioharbor-postgres --test migration_from_zero \
  migrations_from_zero_preserve_least_privilege_roles_and_are_idempotent -- --exact
cargo test --locked -p folioharbor-postgres --test rls_matrix
cargo test --locked -p folioharbor-api --test upload_composition \
  production_upload_composition_uses_postgres_and_local_blob_storage -- --exact
cargo test --locked -p folioharbor-worker --test import_recovery \
  restart_after_catalog_commit_reconciles_once_and_finishes_the_leased_job -- --exact
