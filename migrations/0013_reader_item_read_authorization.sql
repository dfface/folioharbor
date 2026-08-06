CREATE OR REPLACE FUNCTION folioharbor.reader_publication_visible(p_actor uuid, p_item uuid)
RETURNS TABLE(
    library_id uuid,
    item_id uuid,
    manifestation_id uuid,
    package_id uuid,
    blob_id uuid,
    storage_key text,
    parser_profile_version text,
    primary_title text,
    authors text[],
    languages text[],
    resources jsonb,
    reading_order jsonb,
    toc jsonb
)
LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    SELECT holding.library_id,
           item.item_id,
           item.manifestation_id,
           package.package_id,
           package.blob_id,
           location.storage_key,
           package.parser_profile_version,
           work.primary_title,
           work.authors,
           expression.languages,
           COALESCE((
             SELECT jsonb_agg(jsonb_build_object(
               'normalized_href',resource.normalized_href,
               'media_type',resource.media_type
             ) ORDER BY resource.resource_order)
             FROM folioharbor.publication_resources resource
             WHERE resource.package_id=package.package_id
           ),'[]'::jsonb),
           COALESCE((
             SELECT jsonb_agg(jsonb_build_object(
               'normalized_href',unit.locator_href,
               'linear',relation.linear
             ) ORDER BY relation.spine_order)
             FROM folioharbor.manifestation_units relation
             JOIN folioharbor.content_units unit
               ON unit.package_id=relation.package_id
              AND unit.content_unit_id=relation.content_unit_id
             WHERE relation.package_id=package.package_id
               AND relation.manifestation_id=item.manifestation_id
           ),'[]'::jsonb),
           COALESCE((
             SELECT jsonb_agg(jsonb_build_object(
               'label',entry.label,
               'normalized_href',entry.locator_href
             ) ORDER BY entry.toc_order)
             FROM folioharbor.package_toc_entries entry
             WHERE entry.package_id=package.package_id
           ),'[]'::jsonb)
    FROM folioharbor.items item
    JOIN folioharbor.holdings holding
      ON holding.holding_id=item.holding_id
     AND holding.manifestation_id=item.manifestation_id
    JOIN folioharbor.publication_packages package
      ON package.package_id=item.package_id
     AND package.manifestation_id=item.manifestation_id
    JOIN folioharbor.blob_locations location
      ON location.blob_id=package.blob_id
     AND location.state='ready'
    JOIN folioharbor.manifestations manifestation
      ON manifestation.manifestation_id=item.manifestation_id
    JOIN folioharbor.manifestation_expressions manifestation_expression
      ON manifestation_expression.manifestation_id=manifestation.manifestation_id
     AND manifestation_expression.expression_order=0
    JOIN folioharbor.expressions expression USING(expression_id)
    JOIN folioharbor.works work USING(work_id)
    WHERE item.item_id=p_item
      AND item.state='active'
      AND holding.state='active'
      AND p_actor=folioharbor.current_user_id()
      AND session_user='folioharbor_api'
      AND EXISTS (
        SELECT 1
        FROM folioharbor.library_memberships membership
        JOIN folioharbor.role_permissions permission USING(role_code)
        WHERE membership.library_id=holding.library_id
          AND membership.user_id=p_actor
          AND membership.status='active'
          AND permission.permission_code='item.read'
      )
$$;
ALTER FUNCTION folioharbor.reader_publication_visible(uuid,uuid)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.reader_publication_visible(uuid,uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.reader_publication_visible(uuid,uuid) FROM folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.reader_publication_visible(uuid,uuid)
    TO folioharbor_api;

UPDATE folioharbor.schema_metadata
SET schema_version=13, applied_at=clock_timestamp()
WHERE singleton;
