ALTER TABLE folioharbor.user_devices
    ADD CONSTRAINT user_devices_device_user_unique UNIQUE (device_id, user_id);

CREATE TABLE folioharbor.reading_states (
    user_id uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id) ON DELETE CASCADE,
    manifestation_id uuid NOT NULL REFERENCES folioharbor.manifestations(manifestation_id) ON DELETE RESTRICT,
    package_id uuid,
    content_unit_id uuid,
    locator jsonb NOT NULL CHECK (jsonb_typeof(locator) = 'object'),
    version bigint NOT NULL CHECK (version > 0),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (user_id, manifestation_id),
    FOREIGN KEY (package_id, manifestation_id)
      REFERENCES folioharbor.publication_packages(package_id, manifestation_id) ON DELETE RESTRICT,
    FOREIGN KEY (package_id, content_unit_id)
      REFERENCES folioharbor.content_units(package_id, content_unit_id) ON DELETE RESTRICT,
    CHECK (content_unit_id IS NULL OR package_id IS NOT NULL)
);

CREATE TABLE folioharbor.device_reading_states (
    user_id uuid NOT NULL,
    device_id uuid NOT NULL,
    manifestation_id uuid NOT NULL REFERENCES folioharbor.manifestations(manifestation_id) ON DELETE RESTRICT,
    locator jsonb NOT NULL CHECK (jsonb_typeof(locator) = 'object'),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (user_id, device_id, manifestation_id),
    FOREIGN KEY (device_id, user_id)
      REFERENCES folioharbor.user_devices(device_id, user_id) ON DELETE CASCADE
);

CREATE TABLE folioharbor.reading_mutations (
    user_id uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id) ON DELETE CASCADE,
    client_mutation_id uuid NOT NULL,
    manifestation_id uuid NOT NULL REFERENCES folioharbor.manifestations(manifestation_id) ON DELETE RESTRICT,
    device_id uuid NOT NULL,
    outcome text NOT NULL CHECK (outcome IN ('updated', 'conflict')),
    global_package_id uuid,
    global_content_unit_id uuid,
    global_locator jsonb NOT NULL CHECK (jsonb_typeof(global_locator) = 'object'),
    global_version bigint NOT NULL CHECK (global_version > 0),
    global_updated_at timestamptz NOT NULL,
    device_locator jsonb NOT NULL CHECK (jsonb_typeof(device_locator) = 'object'),
    device_updated_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (user_id, client_mutation_id),
    FOREIGN KEY (device_id, user_id)
      REFERENCES folioharbor.user_devices(device_id, user_id) ON DELETE CASCADE,
    FOREIGN KEY (global_package_id, manifestation_id)
      REFERENCES folioharbor.publication_packages(package_id, manifestation_id) ON DELETE RESTRICT,
    FOREIGN KEY (global_package_id, global_content_unit_id)
      REFERENCES folioharbor.content_units(package_id, content_unit_id) ON DELETE RESTRICT,
    CHECK (global_content_unit_id IS NULL OR global_package_id IS NOT NULL)
);

CREATE INDEX reading_mutations_manifestation_idx
    ON folioharbor.reading_mutations(user_id, manifestation_id);

ALTER TABLE folioharbor.reading_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.reading_states FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.device_reading_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.device_reading_states FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.reading_mutations ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.reading_mutations FORCE ROW LEVEL SECURITY;

CREATE POLICY reading_states_owner_access ON folioharbor.reading_states
    USING (current_user = 'folioharbor_owner') WITH CHECK (current_user = 'folioharbor_owner');
CREATE POLICY reading_states_user_access ON folioharbor.reading_states
    USING (user_id = folioharbor.current_user_id())
    WITH CHECK (user_id = folioharbor.current_user_id());
CREATE POLICY device_reading_states_owner_access ON folioharbor.device_reading_states
    USING (current_user = 'folioharbor_owner') WITH CHECK (current_user = 'folioharbor_owner');
CREATE POLICY device_reading_states_user_access ON folioharbor.device_reading_states
    USING (user_id = folioharbor.current_user_id())
    WITH CHECK (user_id = folioharbor.current_user_id());
CREATE POLICY reading_mutations_owner_access ON folioharbor.reading_mutations
    USING (current_user = 'folioharbor_owner') WITH CHECK (current_user = 'folioharbor_owner');
CREATE POLICY reading_mutations_user_access ON folioharbor.reading_mutations
    USING (user_id = folioharbor.current_user_id())
    WITH CHECK (user_id = folioharbor.current_user_id());

CREATE FUNCTION folioharbor.progress_manifestation_readable(p_actor uuid, p_manifestation uuid)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path TO '' AS $$
    SELECT p_actor = folioharbor.current_user_id()
       AND session_user = 'folioharbor_api'
       AND EXISTS (
         SELECT 1
         FROM folioharbor.items item
         JOIN folioharbor.holdings holding USING(holding_id)
         JOIN folioharbor.library_memberships membership
           ON membership.library_id = holding.library_id
          AND membership.user_id = p_actor
          AND membership.status = 'active'
         JOIN folioharbor.role_permissions permission USING(role_code)
         WHERE item.manifestation_id = p_manifestation
           AND item.state = 'active'
           AND holding.state = 'active'
           AND permission.permission_code = 'item.read'
       )
$$;
ALTER FUNCTION folioharbor.progress_manifestation_readable(uuid, uuid) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.progress_manifestation_readable(uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.progress_manifestation_readable(uuid, uuid) FROM folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.progress_manifestation_readable(uuid, uuid) TO folioharbor_api;

GRANT SELECT, INSERT, UPDATE ON folioharbor.reading_states,
    folioharbor.device_reading_states, folioharbor.reading_mutations TO folioharbor_api;

UPDATE folioharbor.schema_metadata
SET schema_version = 16, applied_at = clock_timestamp()
WHERE singleton;
