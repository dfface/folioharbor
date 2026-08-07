-- Task 20 Web contracts: server-authoritative navigation capabilities, detailed
-- invitation acceptance, and an exact upload-to-Item completion target.

ALTER TABLE folioharbor.upload_sessions
    ADD COLUMN result_item_id uuid REFERENCES folioharbor.items(item_id) ON DELETE SET NULL,
    ADD CONSTRAINT upload_result_item_terminal
        CHECK (result_item_id IS NULL OR state IN ('ready', 'duplicate'));

CREATE FUNCTION folioharbor.library_web_visible(p_actor uuid)
RETURNS TABLE(
    library_id uuid,
    name text,
    role_code text,
    reader_download_enabled boolean,
    can_upload boolean,
    can_invite_members boolean,
    can_manage_members boolean,
    can_manage_settings boolean
)
LANGUAGE sql
SECURITY DEFINER
SET search_path TO ''
AS $$
    SELECT library.library_id,
           library.name,
           membership.role_code,
           library.reader_download_enabled,
           EXISTS (
               SELECT 1 FROM folioharbor.role_permissions permission
               WHERE permission.role_code = membership.role_code
                 AND permission.permission_code = 'holding.edit'
           ),
           EXISTS (
               SELECT 1 FROM folioharbor.role_permissions permission
               WHERE permission.role_code = membership.role_code
                 AND permission.permission_code = 'member.invite'
           ),
           EXISTS (
               SELECT 1 FROM folioharbor.role_permissions permission
               WHERE permission.role_code = membership.role_code
                 AND permission.permission_code = 'library.manage'
           ),
           EXISTS (
               SELECT 1 FROM folioharbor.role_permissions permission
               WHERE permission.role_code = membership.role_code
                 AND permission.permission_code = 'library.manage'
           )
      FROM folioharbor.libraries library
      JOIN folioharbor.library_memberships membership USING (library_id)
     WHERE session_user = 'folioharbor_api'
       AND p_actor = folioharbor.current_user_id()
       AND membership.user_id = p_actor
       AND membership.status = 'active'
     ORDER BY library.library_id
$$;

ALTER FUNCTION folioharbor.library_web_visible(uuid) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.library_web_visible(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.library_web_visible(uuid) TO folioharbor_api;

CREATE FUNCTION folioharbor.library_accept_invitation_detailed(
    p_user uuid,
    p_hash bytea,
    p_now timestamptz
)
RETURNS TABLE(outcome text, accepted_library_id uuid, invited_email text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO ''
AS $$
#variable_conflict use_column
DECLARE
    invitation folioharbor.library_invitations%ROWTYPE;
    account_email text;
    account_status text;
BEGIN
    IF session_user <> 'folioharbor_api'
       OR p_user IS DISTINCT FROM folioharbor.current_user_id() THEN
        RAISE EXCEPTION 'invitation actor does not match request context' USING ERRCODE = '22023';
    END IF;

    SELECT invitation_row.* INTO invitation
      FROM folioharbor.library_invitations invitation_row
     WHERE invitation_row.token_hash = p_hash
     FOR UPDATE;
    IF invitation.invitation_id IS NULL THEN
        RETURN QUERY SELECT 'invalid'::text, NULL::uuid, NULL::text;
        RETURN;
    END IF;

    SELECT account.normalized_email, account.status
      INTO account_email, account_status
      FROM folioharbor.user_accounts account
     WHERE account.user_id = p_user
     FOR KEY SHARE;
    IF account_email IS NULL THEN
        RETURN QUERY SELECT 'invalid'::text, NULL::uuid, NULL::text;
        RETURN;
    END IF;
    IF account_status <> 'verified' THEN
        RETURN QUERY SELECT 'unverified'::text, NULL::uuid, NULL::text;
        RETURN;
    END IF;
    IF invitation.consumed_at IS NOT NULL THEN
        RETURN QUERY SELECT 'consumed'::text, NULL::uuid, NULL::text;
        RETURN;
    END IF;
    IF invitation.expires_at <= p_now THEN
        RETURN QUERY SELECT 'expired'::text, NULL::uuid, NULL::text;
        RETURN;
    END IF;
    IF invitation.normalized_email <> account_email THEN
        RETURN QUERY SELECT 'wrong_account'::text, NULL::uuid, invitation.display_email;
        RETURN;
    END IF;

    INSERT INTO folioharbor.library_memberships(
        library_id, user_id, role_code, status, joined_at
    ) VALUES (
        invitation.library_id, p_user, invitation.role_code, 'active', p_now
    ) ON CONFLICT (library_id, user_id) WHERE status = 'active' DO NOTHING;
    UPDATE folioharbor.library_invitations invitation_row
       SET consumed_at = p_now,
           consumed_by = p_user,
           version = invitation_row.version + 1
     WHERE invitation_row.invitation_id = invitation.invitation_id;
    RETURN QUERY SELECT 'accepted'::text, invitation.library_id, NULL::text;
END
$$;

ALTER FUNCTION folioharbor.library_accept_invitation_detailed(uuid, bytea, timestamptz)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.library_accept_invitation_detailed(uuid, bytea, timestamptz)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.library_accept_invitation_detailed(uuid, bytea, timestamptz)
    TO folioharbor_api;

CREATE OR REPLACE FUNCTION folioharbor.catalog_finish_import(
    p_library uuid, p_upload uuid, p_actor uuid, p_blob uuid, p_logical bigint,
    p_profile text, p_item uuid,
    p_audit uuid, p_request text, p_now timestamptz
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE quota_outcome text;
DECLARE completion_outcome text;
BEGIN
    IF NOT folioharbor.catalog_validate_import(
      p_library,p_upload,p_actor,p_blob,p_logical,p_request
    ) THEN RETURN 'not_active'; END IF;
    SELECT CASE WHEN item.source_upload_id=p_upload THEN 'created' ELSE 'duplicate' END
      INTO completion_outcome
      FROM folioharbor.items item
      JOIN folioharbor.holdings holding
        ON holding.holding_id=item.holding_id
       AND holding.manifestation_id=item.manifestation_id
      JOIN folioharbor.publication_packages package
        ON package.package_id=item.package_id
       AND package.manifestation_id=item.manifestation_id
      JOIN folioharbor.item_assets asset
        ON asset.item_id=item.item_id AND asset.asset_kind='original'
      JOIN folioharbor.upload_sessions provenance
        ON provenance.upload_id=item.source_upload_id
      JOIN folioharbor.blobs blob ON blob.blob_id=p_blob
      WHERE item.item_id=p_item AND item.state='active' AND holding.state='active'
        AND holding.library_id=p_library AND package.blob_id=p_blob
        AND package.parser_profile_version=p_profile AND asset.blob_id=p_blob
        AND provenance.library_id=p_library
        AND provenance.sha256 IS NOT DISTINCT FROM blob.sha256
        AND provenance.received_bytes=blob.byte_size
        AND ((item.source_upload_id=p_upload AND provenance.state='importing')
          OR (item.source_upload_id<>p_upload AND provenance.state='ready'))
      FOR UPDATE OF item;
    IF completion_outcome IS NULL THEN RETURN 'not_active'; END IF;
    quota_outcome := CASE WHEN completion_outcome='duplicate'
      THEN folioharbor.quota_release(p_library,p_upload)
      ELSE folioharbor.quota_consume(p_library,p_upload) END;
    IF quota_outcome <> 'applied' THEN RETURN 'not_active'; END IF;
    UPDATE folioharbor.upload_sessions SET
      state=CASE WHEN completion_outcome='duplicate' THEN 'duplicate' ELSE 'ready' END,
      result_item_id=p_item,
      updated_at=p_now WHERE upload_id=p_upload;
    DELETE FROM folioharbor.blob_reachability_candidates
      WHERE source_upload_id=p_upload;
    INSERT INTO folioharbor.audit_events(
      audit_event_id,actor_id,effective_actor_id,library_id,action_code,resource_type,
      resource_id,decision,reason_code,request_id,source,occurred_at,network_hmac
    ) VALUES (p_audit,p_actor,p_actor,p_library,'publication.import','upload',p_upload,
      'allowed',NULL,p_request,'worker',p_now,NULL);
    RETURN completion_outcome;
END $$;

ALTER FUNCTION folioharbor.catalog_finish_import(
    uuid,uuid,uuid,uuid,bigint,text,uuid,uuid,text,timestamptz
) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.catalog_finish_import(
    uuid,uuid,uuid,uuid,bigint,text,uuid,uuid,text,timestamptz
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.catalog_finish_import(
    uuid,uuid,uuid,uuid,bigint,text,uuid,uuid,text,timestamptz
) TO folioharbor_worker;

UPDATE folioharbor.schema_metadata
   SET schema_version = 26, applied_at = clock_timestamp()
 WHERE singleton;
