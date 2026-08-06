CREATE FUNCTION folioharbor.reader_item_read_access(p_actor uuid, p_item uuid)
RETURNS TABLE(
    library_id uuid,
    item_id uuid,
    manifestation_id uuid,
    package_id uuid,
    blob_id uuid,
    storage_key text,
    parser_profile_version text,
    membership_version bigint
)
LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    SELECT holding.library_id,
           item.item_id,
           item.manifestation_id,
           package.package_id,
           package.blob_id,
           location.storage_key,
           package.parser_profile_version,
           membership.version
    FROM folioharbor.items item
    JOIN folioharbor.holdings holding
      ON holding.holding_id=item.holding_id
     AND holding.manifestation_id=item.manifestation_id
    JOIN folioharbor.publication_packages package
      ON package.package_id=item.package_id
     AND package.manifestation_id=item.manifestation_id
    JOIN LATERAL (
      SELECT candidate.storage_key
      FROM folioharbor.blob_locations candidate
      WHERE candidate.blob_id=package.blob_id
        AND candidate.state='ready'
      ORDER BY candidate.storage_key
      LIMIT 1
    ) location ON true
    JOIN folioharbor.library_memberships membership
      ON membership.library_id=holding.library_id
     AND membership.user_id=p_actor
     AND membership.status='active'
    JOIN folioharbor.role_permissions permission
      ON permission.role_code=membership.role_code
     AND permission.permission_code='item.read'
    WHERE item.item_id=p_item
      AND item.state='active'
      AND holding.state='active'
      AND p_actor=folioharbor.current_user_id()
      AND session_user='folioharbor_api'
$$;
ALTER FUNCTION folioharbor.reader_item_read_access(uuid,uuid)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.reader_item_read_access(uuid,uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.reader_item_read_access(uuid,uuid) FROM folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.reader_item_read_access(uuid,uuid)
    TO folioharbor_api;

UPDATE folioharbor.schema_metadata
SET schema_version=14, applied_at=clock_timestamp()
WHERE singleton;
