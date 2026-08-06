CREATE INDEX holdings_active_library_keyset_idx
    ON folioharbor.holdings(library_id, holding_id DESC)
    WHERE state='active';

CREATE INDEX items_active_holding_canonical_idx
    ON folioharbor.items(holding_id, created_at DESC, item_id DESC)
    WHERE state='active';

CREATE FUNCTION folioharbor.catalog_item_projection_visible(
    p_actor uuid, p_library uuid, p_item uuid, p_membership_version bigint
) RETURNS TABLE(
    holding_id uuid,
    item_id uuid,
    package_id uuid,
    manifestation_id uuid,
    primary_title text,
    authors text[],
    languages text[],
    identifiers text[],
    media_type text
)
LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    SELECT holding.holding_id,
           item.item_id,
           package.package_id,
           holding.manifestation_id,
           work.primary_title,
           work.authors,
           expression.languages,
           manifestation.identifiers,
           upload.media_type
    FROM folioharbor.holdings holding
    JOIN folioharbor.items item
      ON item.holding_id=holding.holding_id
     AND item.manifestation_id=holding.manifestation_id
    JOIN folioharbor.publication_packages package
      ON package.package_id=item.package_id
     AND package.manifestation_id=item.manifestation_id
    JOIN folioharbor.manifestations manifestation
      ON manifestation.manifestation_id=item.manifestation_id
    JOIN folioharbor.manifestation_expressions relation
      ON relation.manifestation_id=manifestation.manifestation_id
     AND relation.expression_order=0
    JOIN folioharbor.expressions expression USING(expression_id)
    JOIN folioharbor.works work USING(work_id)
    JOIN folioharbor.upload_sessions upload
      ON upload.upload_id=item.source_upload_id
     AND upload.library_id=holding.library_id
    WHERE holding.library_id=p_library
      AND holding.state='active'
      AND item.item_id=p_item
      AND item.state='active'
      AND p_actor=folioharbor.current_user_id()
      AND p_library=folioharbor.current_library_id()
      AND EXISTS (
        SELECT 1
        FROM folioharbor.library_memberships membership
        JOIN folioharbor.role_permissions permission USING(role_code)
        WHERE membership.library_id=p_library
          AND membership.user_id=p_actor
          AND membership.status='active'
          AND membership.version=p_membership_version
          AND permission.permission_code='holding.view'
      )
$$;
ALTER FUNCTION folioharbor.catalog_item_projection_visible(uuid,uuid,uuid,bigint)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.catalog_item_projection_visible(uuid,uuid,uuid,bigint)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.catalog_item_projection_visible(uuid,uuid,uuid,bigint)
    TO folioharbor_api;

UPDATE folioharbor.schema_metadata
SET schema_version=11, applied_at=clock_timestamp()
WHERE singleton;
