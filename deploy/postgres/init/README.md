# PostgreSQL initialization

These scripts run only when the PostgreSQL data directory is empty. `001-roles.sql` creates the three non-superuser, non-`BYPASSRLS` roles. `002-role-passwords.sh` reads Docker secret files, assigns distinct passwords, and transfers ownership of the `folioharbor` database to `folioharbor_owner` so the migration CLI can create the application schema.

The owner credential is for `folioharbor migrate`, `folioharbor admin create`, backup/restore, and `folioharbor storage check` only. API and Worker must use their distinct runtime credentials. Initialization scripts are not a rotation mechanism; rotate credentials with an authenticated PostgreSQL administrative session and update the corresponding secret and database-URL files together.
