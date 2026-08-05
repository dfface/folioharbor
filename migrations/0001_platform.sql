CREATE SCHEMA folioharbor AUTHORIZATION folioharbor_owner;
REVOKE ALL ON SCHEMA folioharbor FROM PUBLIC;

CREATE TABLE folioharbor.schema_metadata (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    schema_version bigint NOT NULL,
    applied_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

INSERT INTO folioharbor.schema_metadata (singleton, schema_version)
VALUES (true, 1);

CREATE FUNCTION folioharbor.current_user_id()
RETURNS uuid
LANGUAGE sql
STABLE
SET search_path TO ''
RETURN NULLIF(current_setting('app.user_id', true), '')::uuid;

CREATE FUNCTION folioharbor.current_library_id()
RETURNS uuid
LANGUAGE sql
STABLE
SET search_path TO ''
RETURN NULLIF(current_setting('app.library_id', true), '')::uuid;

CREATE FUNCTION folioharbor.current_request_id()
RETURNS text
LANGUAGE sql
STABLE
SET search_path TO ''
RETURN NULLIF(current_setting('app.request_id', true), '');

CREATE FUNCTION folioharbor.is_worker()
RETURNS boolean
LANGUAGE sql
STABLE
SET search_path TO ''
RETURN COALESCE(NULLIF(current_setting('app.is_worker', true), '')::boolean, false);

ALTER FUNCTION folioharbor.current_user_id() OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.current_library_id() OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.current_request_id() OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.is_worker() OWNER TO folioharbor_owner;

REVOKE ALL ON FUNCTION folioharbor.current_user_id() FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.current_library_id() FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.current_request_id() FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.is_worker() FROM PUBLIC;

GRANT USAGE ON SCHEMA folioharbor TO folioharbor_api, folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.current_user_id() TO folioharbor_api, folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.current_library_id() TO folioharbor_api, folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.current_request_id() TO folioharbor_api, folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.is_worker() TO folioharbor_api, folioharbor_worker;
