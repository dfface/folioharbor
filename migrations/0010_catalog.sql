CREATE TABLE folioharbor.works (
    work_id uuid PRIMARY KEY,
    primary_title text NOT NULL CHECK (length(primary_title) BETWEEN 1 AND 2048),
    authors text[] NOT NULL,
    created_at timestamptz NOT NULL
);

CREATE TABLE folioharbor.expressions (
    expression_id uuid PRIMARY KEY,
    work_id uuid NOT NULL REFERENCES folioharbor.works(work_id) ON DELETE RESTRICT,
    languages text[] NOT NULL,
    created_at timestamptz NOT NULL
);

CREATE TABLE folioharbor.manifestations (
    manifestation_id uuid PRIMARY KEY,
    identifiers text[] NOT NULL,
    created_at timestamptz NOT NULL
);

CREATE TABLE folioharbor.manifestation_expressions (
    manifestation_id uuid NOT NULL REFERENCES folioharbor.manifestations(manifestation_id) ON DELETE CASCADE,
    expression_id uuid NOT NULL REFERENCES folioharbor.expressions(expression_id) ON DELETE RESTRICT,
    expression_order integer NOT NULL CHECK (expression_order >= 0),
    PRIMARY KEY (manifestation_id, expression_id),
    UNIQUE (manifestation_id, expression_order)
);

CREATE TABLE folioharbor.publication_packages (
    package_id uuid PRIMARY KEY,
    manifestation_id uuid NOT NULL REFERENCES folioharbor.manifestations(manifestation_id) ON DELETE RESTRICT,
    blob_id uuid NOT NULL REFERENCES folioharbor.blobs(blob_id) ON DELETE RESTRICT,
    parser_profile_version text NOT NULL CHECK (length(parser_profile_version) BETWEEN 1 AND 128),
    created_at timestamptz NOT NULL,
    UNIQUE (blob_id, parser_profile_version)
);

CREATE TABLE folioharbor.publication_resources (
    package_id uuid NOT NULL REFERENCES folioharbor.publication_packages(package_id) ON DELETE CASCADE,
    resource_order integer NOT NULL CHECK (resource_order >= 0),
    normalized_href text NOT NULL CHECK (length(normalized_href) BETWEEN 1 AND 2048),
    media_type text NOT NULL CHECK (length(media_type) BETWEEN 1 AND 255),
    source_blob_id uuid NOT NULL REFERENCES folioharbor.blobs(blob_id) ON DELETE RESTRICT,
    PRIMARY KEY (package_id, resource_order),
    UNIQUE (package_id, normalized_href)
);

CREATE TABLE folioharbor.content_units (
    content_unit_id uuid PRIMARY KEY,
    package_id uuid NOT NULL REFERENCES folioharbor.publication_packages(package_id) ON DELETE CASCADE,
    locator_href text NOT NULL CHECK (length(locator_href) BETWEEN 1 AND 2048),
    created_at timestamptz NOT NULL
);

CREATE TABLE folioharbor.manifestation_units (
    manifestation_id uuid NOT NULL REFERENCES folioharbor.manifestations(manifestation_id) ON DELETE CASCADE,
    content_unit_id uuid NOT NULL REFERENCES folioharbor.content_units(content_unit_id) ON DELETE CASCADE,
    spine_order integer NOT NULL CHECK (spine_order >= 0),
    linear boolean NOT NULL,
    PRIMARY KEY (manifestation_id, content_unit_id),
    UNIQUE (manifestation_id, spine_order)
);

CREATE TABLE folioharbor.package_toc_entries (
    package_id uuid NOT NULL REFERENCES folioharbor.publication_packages(package_id) ON DELETE CASCADE,
    toc_order integer NOT NULL CHECK (toc_order >= 0),
    label text NOT NULL CHECK (length(label) BETWEEN 1 AND 2048),
    locator_href text NOT NULL CHECK (length(locator_href) BETWEEN 1 AND 2048),
    PRIMARY KEY (package_id, toc_order)
);

CREATE TABLE folioharbor.holdings (
    holding_id uuid PRIMARY KEY,
    library_id uuid NOT NULL REFERENCES folioharbor.libraries(library_id) ON DELETE CASCADE,
    manifestation_id uuid NOT NULL REFERENCES folioharbor.manifestations(manifestation_id) ON DELETE RESTRICT,
    state text NOT NULL CHECK (state IN ('active','deleted')),
    created_at timestamptz NOT NULL,
    deleted_at timestamptz,
    CHECK ((state='deleted')=(deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX holdings_one_active_manifestation_per_library
    ON folioharbor.holdings(library_id, manifestation_id) WHERE state='active';

CREATE TABLE folioharbor.items (
    item_id uuid PRIMARY KEY,
    holding_id uuid NOT NULL REFERENCES folioharbor.holdings(holding_id) ON DELETE CASCADE,
    state text NOT NULL CHECK (state IN ('active','deleted')),
    created_at timestamptz NOT NULL,
    deleted_at timestamptz,
    CHECK ((state='deleted')=(deleted_at IS NOT NULL))
);

CREATE TABLE folioharbor.item_assets (
    item_id uuid NOT NULL REFERENCES folioharbor.items(item_id) ON DELETE CASCADE,
    blob_id uuid NOT NULL REFERENCES folioharbor.blobs(blob_id) ON DELETE RESTRICT,
    asset_kind text NOT NULL CHECK (asset_kind IN ('original')),
    created_at timestamptz NOT NULL,
    PRIMARY KEY (item_id, asset_kind)
);
CREATE INDEX item_assets_original_blob ON folioharbor.item_assets(blob_id) WHERE asset_kind='original';

CREATE TABLE folioharbor.manifestation_assets (
    manifestation_id uuid NOT NULL REFERENCES folioharbor.manifestations(manifestation_id) ON DELETE CASCADE,
    blob_id uuid NOT NULL REFERENCES folioharbor.blobs(blob_id) ON DELETE RESTRICT,
    asset_kind text NOT NULL CHECK (asset_kind IN ('original','cover')),
    locator_href text,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (manifestation_id, asset_kind),
    CHECK ((asset_kind='cover')=(locator_href IS NOT NULL))
);

ALTER TABLE folioharbor.holdings ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.holdings FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.items ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.items FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.item_assets ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.item_assets FORCE ROW LEVEL SECURITY;

CREATE POLICY holdings_owner_access ON folioharbor.holdings
    USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY holdings_runtime_access ON folioharbor.holdings
    USING (library_id=folioharbor.current_library_id() AND (folioharbor.is_worker() OR EXISTS (
        SELECT 1 FROM folioharbor.library_memberships membership
        WHERE membership.library_id=holdings.library_id
          AND membership.user_id=folioharbor.current_user_id() AND membership.status='active')))
    WITH CHECK (library_id=folioharbor.current_library_id() AND folioharbor.is_worker());
CREATE POLICY items_owner_access ON folioharbor.items
    USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY items_runtime_access ON folioharbor.items
    USING (EXISTS (SELECT 1 FROM folioharbor.holdings holding
        WHERE holding.holding_id=items.holding_id
          AND holding.library_id=folioharbor.current_library_id()))
    WITH CHECK (folioharbor.is_worker() AND EXISTS (SELECT 1 FROM folioharbor.holdings holding
        WHERE holding.holding_id=items.holding_id
          AND holding.library_id=folioharbor.current_library_id()));
CREATE POLICY item_assets_owner_access ON folioharbor.item_assets
    USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY item_assets_runtime_access ON folioharbor.item_assets
    USING (EXISTS (SELECT 1 FROM folioharbor.items item JOIN folioharbor.holdings holding USING(holding_id)
        WHERE item.item_id=item_assets.item_id AND holding.library_id=folioharbor.current_library_id()))
    WITH CHECK (folioharbor.is_worker() AND EXISTS (SELECT 1 FROM folioharbor.items item JOIN folioharbor.holdings holding USING(holding_id)
        WHERE item.item_id=item_assets.item_id AND holding.library_id=folioharbor.current_library_id()));

REVOKE ALL ON folioharbor.works, folioharbor.expressions, folioharbor.manifestations,
    folioharbor.manifestation_expressions, folioharbor.publication_packages,
    folioharbor.publication_resources, folioharbor.content_units,
    folioharbor.manifestation_units, folioharbor.package_toc_entries,
    folioharbor.manifestation_assets FROM PUBLIC;
REVOKE ALL ON folioharbor.holdings, folioharbor.items, folioharbor.item_assets FROM PUBLIC;
GRANT SELECT,INSERT ON folioharbor.works, folioharbor.expressions, folioharbor.manifestations,
    folioharbor.manifestation_expressions, folioharbor.publication_packages,
    folioharbor.publication_resources, folioharbor.content_units,
    folioharbor.manifestation_units, folioharbor.package_toc_entries,
    folioharbor.manifestation_assets TO folioharbor_worker;
GRANT SELECT ON folioharbor.blobs TO folioharbor_worker;
GRANT SELECT,INSERT,UPDATE ON folioharbor.holdings, folioharbor.items, folioharbor.item_assets TO folioharbor_worker;
GRANT SELECT ON folioharbor.holdings, folioharbor.items, folioharbor.item_assets TO folioharbor_api;

ALTER TABLE folioharbor.audit_events DROP CONSTRAINT audit_events_action_code_check;
ALTER TABLE folioharbor.audit_events ADD CONSTRAINT audit_events_action_code_check
    CHECK(action_code IN('library.view','library.manage','member.invite','member.role.change','member.remove','publication.import'));
ALTER TABLE folioharbor.audit_events DROP CONSTRAINT audit_events_resource_type_check;
ALTER TABLE folioharbor.audit_events ADD CONSTRAINT audit_events_resource_type_check
    CHECK(resource_type IN('library','membership','invitation','upload'));

CREATE FUNCTION folioharbor.catalog_finish_import(
    p_library uuid, p_upload uuid, p_actor uuid, p_item uuid, p_duplicate boolean,
    p_audit uuid, p_request text, p_now timestamptz
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE quota_outcome text;
BEGIN
    IF NOT folioharbor.is_worker()
       OR p_library IS DISTINCT FROM folioharbor.current_library_id()
       OR p_request IS DISTINCT FROM folioharbor.current_request_id()
       OR session_user <> 'folioharbor_worker' THEN
        RETURN 'not_active';
    END IF;
    PERFORM 1 FROM folioharbor.upload_sessions
      WHERE upload_id=p_upload AND library_id=p_library AND created_by=p_actor
        AND state='importing' FOR UPDATE;
    IF NOT FOUND THEN RETURN 'not_active'; END IF;
    IF NOT EXISTS (
      SELECT 1 FROM folioharbor.items item JOIN folioharbor.holdings holding USING(holding_id)
      WHERE item.item_id=p_item AND item.state='active' AND holding.state='active'
        AND holding.library_id=p_library
    ) THEN RETURN 'not_active'; END IF;
    quota_outcome := CASE WHEN p_duplicate
      THEN folioharbor.quota_release(p_library,p_upload)
      ELSE folioharbor.quota_consume(p_library,p_upload) END;
    IF quota_outcome <> 'applied' THEN RETURN 'not_active'; END IF;
    UPDATE folioharbor.upload_sessions SET
      state=CASE WHEN p_duplicate THEN 'duplicate' ELSE 'ready' END,
      updated_at=p_now WHERE upload_id=p_upload;
    INSERT INTO folioharbor.audit_events(
      audit_event_id,actor_id,effective_actor_id,library_id,action_code,resource_type,
      resource_id,decision,reason_code,request_id,source,occurred_at,network_hmac
    ) VALUES (p_audit,p_actor,p_actor,p_library,'publication.import','upload',p_upload,
      'allowed',NULL,p_request,'worker',p_now,NULL);
    RETURN 'applied';
END $$;
ALTER FUNCTION folioharbor.catalog_finish_import(uuid,uuid,uuid,uuid,boolean,uuid,text,timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.catalog_finish_import(uuid,uuid,uuid,uuid,boolean,uuid,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.catalog_finish_import(uuid,uuid,uuid,uuid,boolean,uuid,text,timestamptz) TO folioharbor_worker;

CREATE FUNCTION folioharbor.catalog_item_visible(
    p_actor uuid, p_library uuid, p_item uuid, p_membership_version bigint
) RETURNS TABLE(item_id uuid, package_id uuid, manifestation_id uuid, primary_title text)
LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    SELECT item.item_id, package.package_id, holding.manifestation_id, work.primary_title
    FROM folioharbor.items item
    JOIN folioharbor.holdings holding USING(holding_id)
    JOIN folioharbor.publication_packages package USING(manifestation_id)
    JOIN folioharbor.manifestation_expressions relation USING(manifestation_id)
    JOIN folioharbor.expressions expression USING(expression_id)
    JOIN folioharbor.works work USING(work_id)
    WHERE item.item_id=p_item AND item.state='active' AND holding.state='active'
      AND holding.library_id=p_library
      AND p_actor=folioharbor.current_user_id()
      AND p_library=folioharbor.current_library_id()
      AND EXISTS (
        SELECT 1 FROM folioharbor.library_memberships membership
        JOIN folioharbor.role_permissions permission USING(role_code)
        WHERE membership.library_id=p_library AND membership.user_id=p_actor
          AND membership.status='active' AND membership.version=p_membership_version
          AND permission.permission_code='holding.view')
$$;
ALTER FUNCTION folioharbor.catalog_item_visible(uuid,uuid,uuid,bigint) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.catalog_item_visible(uuid,uuid,uuid,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.catalog_item_visible(uuid,uuid,uuid,bigint) TO folioharbor_api;

UPDATE folioharbor.schema_metadata SET schema_version=10,applied_at=clock_timestamp() WHERE singleton;
