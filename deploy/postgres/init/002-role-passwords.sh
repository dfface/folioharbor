#!/bin/sh
set -eu

read_secret() {
    variable_name="$1"
    eval "secret_file=\${${variable_name}_FILE:-}"
    if [ -z "$secret_file" ] || [ ! -r "$secret_file" ]; then
        echo "required secret file for $variable_name is unavailable" >&2
        exit 1
    fi
    secret_value="$(cat "$secret_file")"
    if [ -z "$secret_value" ]; then
        echo "required secret file for $variable_name is empty" >&2
        exit 1
    fi
    printf '%s' "$secret_value"
}

FH_OWNER_PASSWORD="$(read_secret FOLIOHARBOR_OWNER_PASSWORD)"
FH_API_PASSWORD="$(read_secret FOLIOHARBOR_API_PASSWORD)"
FH_WORKER_PASSWORD="$(read_secret FOLIOHARBOR_WORKER_PASSWORD)"
export FH_OWNER_PASSWORD FH_API_PASSWORD FH_WORKER_PASSWORD

psql --set=ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<'SQL'
\getenv owner_password FH_OWNER_PASSWORD
\getenv api_password FH_API_PASSWORD
\getenv worker_password FH_WORKER_PASSWORD
ALTER ROLE folioharbor_owner PASSWORD :'owner_password';
ALTER ROLE folioharbor_api PASSWORD :'api_password';
ALTER ROLE folioharbor_worker PASSWORD :'worker_password';
ALTER DATABASE folioharbor OWNER TO folioharbor_owner;
SQL

unset FH_OWNER_PASSWORD FH_API_PASSWORD FH_WORKER_PASSWORD
