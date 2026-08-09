# Configuration operations

Configuration precedence is built-in defaults, TOML, `FOLIOHARBOR_*` environment variables, then explicit CLI overrides. Configuration is validated once at process start and is not hot-reloaded. API and Worker emit JSON logs; `FOLIOHARBOR_OBSERVABILITY_LOG_FILTER` controls filtering. Set `FOLIOHARBOR_OBSERVABILITY_OTLP_ENDPOINT` to an HTTP(S) OTLP/gRPC collector URL to export traces and metrics, or leave it unset/empty to disable exporting.

Required production values include a role-specific database URL, an application-secret key ID, an application secret of at least 32 bytes, an HTTPS public base URL, one absolute storage root, and valid SMTP configuration whenever verification, invitation, or password-reset mail is enabled. The storage root contains managed `objects/` and transient `staging/` subdirectories; do not configure or mount them independently. A transient SMTP outage does not make existing reading unavailable; missing required SMTP configuration does.

Sensitive settings support either `NAME` or `NAME_FILE`, never both. This includes database URLs, the current/old application secrets, and mail passwords. Prefer a secret manager or Compose secrets with mode `0600`. Never put credentials in TOML, command arguments, image layers, logs, issue reports, or the committed example environment. `folioharbor admin create` accepts no password argument, password file, or administrator-password environment variable; it reads and confirms the password on a TTY.

Use three PostgreSQL roles and credentials:

- `folioharbor_owner`: migrations, bootstrap, backup/restore, and consistency checks only.
- `folioharbor_api`: API runtime only.
- `folioharbor_worker`: Worker runtime only.

Neither runtime role is superuser, database owner, `BYPASSRLS`, or a substitute for the other. A system administrator row is separate from library membership and grants no implicit `item.read` or other content permission.

Readiness is intentionally aggregate. `/health/live` checks only the serving process. `/health/ready` returns `ready`, `bootstrap_required`, or `unavailable`; it never names the failed database, schema, storage path, reserve, or credential. Readiness requires database access, exact schema 28 compatibility, valid required configuration, usable Blob storage above the configured free reserve, and at least one system administrator.

## EPUB import compatibility

FolioHarbor imports non-DRM EPUB 2.0, EPUB 2.0.1, and EPUB 3.0 through 3.3
packages. It accepts the common recoverable package variations used by these
versions: an EPUB 2 NCX selected by `spine@toc`, a single unambiguous NCX when
that attribute is absent, EPUB 2 cover metadata or guide-cover fallback, and a
readable spine-derived table of contents only when no navigation alternative
is declared. A declared Navigation Document or NCX that is unusable is rejected
rather than being replaced with a spine-derived table of contents. EPUB 3
Navigation Documents take precedence when they are usable.

This compatibility is intentionally bounded. Encrypted ZIP entries and
DRM-protected EPUBs are not supported and are rejected. Archives that are
damaged, exceed import safety limits, contain unsafe paths or navigation
targets, or have ambiguous or unusable package metadata may also be rejected;
the importer does not promise recovery for arbitrary malformed EPUB archives.
