#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
temporary_directory="$(mktemp -d)"
failures=0

cleanup() {
  rm -rf "$temporary_directory"
}

trap cleanup EXIT INT TERM

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  failures=$((failures + 1))
}

assert_equals() {
  expected="$1"
  actual="$2"
  description="$3"

  if [[ "$actual" != "$expected" ]]; then
    fail "$description: expected '$expected', got '$actual'"
  fi
}

template_path="$repository_root/deploy/staging.env.example"
if [[ ! -f "$template_path" ]]; then
  fail "staging template exists at deploy/staging.env.example"
else
  fixture_env="$temporary_directory/staging.env"
  cp "$template_path" "$fixture_env"

  read_fixture_value() {
    variable_name="$1"
    bash -c 'set -a; . "$1"; set +a; printenv "$2"' \
      bash "$fixture_env" "$variable_name"
  }

  assert_equals "folioharbor-staging" \
    "$(read_fixture_value FOLIOHARBOR_COMPOSE_PROJECT)" \
    "staging Compose project is isolated"
  assert_equals "./secrets.staging/postgres-password" \
    "$(read_fixture_value FOLIOHARBOR_POSTGRES_PASSWORD_FILE)" \
    "staging PostgreSQL secret path is isolated"
  assert_equals "./secrets.staging/application-secret" \
    "$(read_fixture_value FOLIOHARBOR_APPLICATION_SECRET_FILE)" \
    "staging application secret path is isolated"
  assert_equals "https://staging-library.example/" \
    "$(read_fixture_value FOLIOHARBOR_PUBLIC_BASE_URL)" \
    "staging public URL is configured"
fi

if ! git -C "$repository_root" check-ignore -q deploy/.env.staging; then
  fail "deploy/.env.staging is ignored"
fi

if ! git -C "$repository_root" check-ignore -q deploy/secrets.staging/application-secret; then
  fail "deploy/secrets.staging/ is ignored"
fi

if (( failures > 0 )); then
  exit 1
fi

printf 'PASS: staging configuration contract\n'
