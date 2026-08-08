#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
temporary_directory="$(mktemp -d)"
failures=0
smoke_script="$repository_root/scripts/smoke.sh"

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

assert_contains() {
  needle="$1"
  haystack="$2"
  description="$3"

  if [[ "$haystack" != *"$needle"* ]]; then
    fail "$description: expected output to contain '$needle'"
  fi
}

assert_empty_file() {
  file_path="$1"
  description="$2"

  if [[ -s "$file_path" ]]; then
    fail "$description: expected no Docker invocation"
  fi
}

run_smoke() {
  smoke_output=""
  if smoke_output="$("$smoke_script" "$@" 2>&1)"; then
    smoke_status=0
  else
    smoke_status=$?
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

fixture_secrets="$temporary_directory/secrets.staging"
fixture_env="$temporary_directory/staging.env"
fake_bin="$temporary_directory/bin"
docker_log="$temporary_directory/docker.log"
mkdir -p "$fixture_secrets" "$fake_bin"
chmod 700 "$fixture_secrets"

for secret in postgres-password owner-password api-password worker-password \
  owner-database-url api-database-url worker-database-url application-secret; do
  printf 'fixture-only-value' > "$fixture_secrets/$secret"
  chmod 600 "$fixture_secrets/$secret"
done

awk -v secret_directory="$fixture_secrets" \
  '{ gsub("\\./secrets\\.staging", secret_directory); print }' \
  "$repository_root/deploy/staging.env.example" > "$fixture_env"

cat > "$fake_bin/docker" <<'EOF'
#!/usr/bin/env bash
printf '%s ' "$@" >> "$SMOKE_TEST_DOCKER_LOG"
printf '\n' >> "$SMOKE_TEST_DOCKER_LOG"
EOF
chmod 700 "$fake_bin/docker"

if [[ ! -x "$smoke_script" ]]; then
  fail "smoke script exists and is executable"
else
  : > "$docker_log"
  SMOKE_TEST_DOCKER_LOG="$docker_log" PATH="$fake_bin:$PATH" \
    run_smoke --env-file "$fixture_env" check
  if (( smoke_status != 0 )); then
    fail "check succeeds for complete staging configuration: $smoke_output"
  fi
  assert_contains \
    "compose --env-file $fixture_env --project-name folioharbor-staging -f $repository_root/deploy/compose.yaml config --quiet" \
    "$(cat "$docker_log")" \
    "check renders the selected staging Compose configuration"

  rm "$fixture_secrets/api-password"
  : > "$docker_log"
  SMOKE_TEST_DOCKER_LOG="$docker_log" PATH="$fake_bin:$PATH" \
    run_smoke --env-file "$fixture_env" check
  if (( smoke_status == 0 )); then
    fail "check rejects a missing secret file"
  fi
  assert_contains "missing or empty secret file" "$smoke_output" \
    "missing secret failure explains the operator action"

  : > "$docker_log"
  SMOKE_TEST_DOCKER_LOG="$docker_log" PATH="$fake_bin:$PATH" \
    run_smoke --env-file "$fixture_env" smoke
  if (( smoke_status != 0 )); then
    fail "smoke prints the manual checklist: $smoke_output"
  fi
  assert_empty_file "$docker_log" "smoke"
  assert_contains "Upload a representative EPUB" "$smoke_output" \
    "smoke prints the EPUB acceptance step"
fi

if (( failures > 0 )); then
  exit 1
fi

printf 'PASS: staging configuration contract\n'
