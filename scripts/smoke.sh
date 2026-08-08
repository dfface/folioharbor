#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
deploy_directory="$repository_root/deploy"
compose_file="$deploy_directory/compose.yaml"
environment_file="$deploy_directory/.env.staging"

secret_variables=(
  FOLIOHARBOR_POSTGRES_PASSWORD_FILE
  FOLIOHARBOR_OWNER_PASSWORD_FILE
  FOLIOHARBOR_API_PASSWORD_FILE
  FOLIOHARBOR_WORKER_PASSWORD_FILE
  FOLIOHARBOR_OWNER_DATABASE_URL_FILE
  FOLIOHARBOR_API_DATABASE_URL_FILE
  FOLIOHARBOR_WORKER_DATABASE_URL_FILE
  FOLIOHARBOR_APPLICATION_SECRET_FILE
)

usage() {
  cat <<'EOF'
Usage: scripts/smoke.sh [--env-file PATH] <command>

Commands:
  check    Validate staging configuration without starting containers.
  up       Start staging services and create an administrator.
  status   Show staging service state.
  logs     Follow all staging logs or one service's logs.
  down     Stop staging services while preserving volumes.
  destroy  Remove staging services and volumes after confirmation.
  smoke    Print the manual EPUB smoke checklist without calling Docker.
EOF
}

die() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

resolve_environment_file() {
  case "$environment_file" in
    /*) ;;
    *)
      case "$environment_file" in
        *".."*) die "relative --env-file must not contain .." ;;
      esac
      environment_file="$repository_root/$environment_file"
      ;;
  esac
}

load_environment() {
  [[ -f "$environment_file" ]] || die "environment file does not exist: $environment_file"
  set -a
  # shellcheck disable=SC1090
  . "$environment_file"
  set +a
}

resolve_secret_path() {
  secret_path="$1"
  case "$secret_path" in
    /*) printf '%s\n' "$secret_path" ;;
    *) printf '%s/%s\n' "$deploy_directory" "$secret_path" ;;
  esac
}

permission_mode() {
  stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1"
}

require_restrictive_permissions() {
  path="$1"
  path_kind="$2"
  mode="$(permission_mode "$path")" || die "could not read permissions for $path"

  case "$mode" in
    [0-7]00|[0-7]000) ;;
    *) die "$path_kind must have mode 0700 or stricter: $path has $mode" ;;
  esac
}

validate_secret_file() {
  variable_name="$1"
  secret_value="${!variable_name:-}"
  [[ -n "$secret_value" ]] || die "missing $variable_name in $environment_file"

  case "$secret_value" in
    *secrets.staging/*) ;;
    *) die "$variable_name must point into secrets.staging: $secret_value" ;;
  esac

  secret_path="$(resolve_secret_path "$secret_value")"
  [[ -f "$secret_path" && -s "$secret_path" ]] || \
    die "missing or empty secret file for $variable_name: $secret_path"
  require_restrictive_permissions "$(dirname "$secret_path")" "secret directory"
  require_restrictive_permissions "$secret_path" "secret file"
}

compose() {
  docker compose --env-file "$environment_file" \
    --project-name "$FOLIOHARBOR_COMPOSE_PROJECT" \
    -f "$compose_file" "$@"
}

prepare_compose() {
  command -v docker >/dev/null 2>&1 || die "Docker Compose requires docker"
  load_environment
  [[ -n "${FOLIOHARBOR_COMPOSE_PROJECT:-}" ]] || \
    die "missing FOLIOHARBOR_COMPOSE_PROJECT in $environment_file"
}

check() {
  prepare_compose

  for variable_name in "${secret_variables[@]}"; do
    validate_secret_file "$variable_name"
  done

  compose config --quiet
  printf 'staging configuration is valid: %s\n' "$FOLIOHARBOR_COMPOSE_PROJECT"
}

up() {
  admin_email="$1"
  check

  if ! compose up -d postgres storage-init migration; then
    die "failed to start PostgreSQL, storage initialization, or migration; run scripts/smoke.sh --env-file $environment_file logs"
  fi

  if ! compose run --rm migration folioharbor admin create --email "$admin_email"; then
    die "failed to create the staging administrator; run scripts/smoke.sh --env-file $environment_file logs"
  fi

  if ! compose up -d --wait api worker web; then
    die "failed to start healthy API, Worker, and Web services; run scripts/smoke.sh --env-file $environment_file logs"
  fi

  compose ps
  smoke
}

status() {
  prepare_compose
  compose ps
}

logs() {
  service_name="${1:-}"
  prepare_compose

  if [[ -n "$service_name" && "$service_name" == -* ]]; then
    die "service name must not start with -"
  fi

  if [[ -n "$service_name" ]]; then
    compose logs --follow --tail 200 "$service_name"
  else
    compose logs --follow --tail 200
  fi
}

down() {
  prepare_compose
  compose down --remove-orphans
}

destroy() {
  prepare_compose
  printf 'Type %s to permanently remove staging volumes: ' "$FOLIOHARBOR_COMPOSE_PROJECT" >&2
  IFS= read -r confirmation || die "destroy confirmation was not provided"
  [[ "$confirmation" == "$FOLIOHARBOR_COMPOSE_PROJECT" ]] || \
    die "destroy confirmation did not match $FOLIOHARBOR_COMPOSE_PROJECT"
  compose down -v --remove-orphans
}

smoke() {
  if [[ -f "$environment_file" ]]; then
    load_environment
    printf 'Staging URL: %s\n' "${FOLIOHARBOR_PUBLIC_BASE_URL:-not configured}"
  fi

  cat <<'EOF'
Manual EPUB smoke checklist:
1. Sign in with the staging administrator.
2. Upload a representative EPUB and wait for it to appear in the library.
3. Open the book in the reader and navigate between chapters.
4. Refresh the reader, reopen the book, and confirm reading progress persists.
5. Return to the library and confirm the book and progress remain visible.
6. Check `scripts/smoke.sh status` and review service logs if any step fails.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --env-file)
      [[ $# -ge 2 ]] || die "--env-file requires a path"
      environment_file="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *) break ;;
  esac
done

resolve_environment_file

command_name="${1:-}"
shift || true

case "$command_name" in
  check)
    [[ $# -eq 0 ]] || die "check does not accept additional arguments"
    check
    ;;
  up)
    [[ $# -eq 2 && "$1" == "--admin-email" && -n "$2" ]] || \
      die "up requires --admin-email EMAIL"
    up "$2"
    ;;
  status)
    [[ $# -eq 0 ]] || die "status does not accept additional arguments"
    status
    ;;
  logs)
    [[ $# -le 1 ]] || die "logs accepts at most one service name"
    logs "${1:-}"
    ;;
  down)
    [[ $# -eq 0 ]] || die "down does not accept additional arguments"
    down
    ;;
  destroy)
    [[ $# -eq 0 ]] || die "destroy does not accept additional arguments"
    destroy
    ;;
  smoke)
    [[ $# -eq 0 ]] || die "smoke does not accept additional arguments"
    smoke
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac
