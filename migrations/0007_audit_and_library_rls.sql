ALTER TABLE folioharbor.libraries ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.libraries FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.library_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.library_memberships FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.library_invitations ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.library_invitations FORCE ROW LEVEL SECURITY;

CREATE POLICY libraries_owner_access ON folioharbor.libraries USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY memberships_owner_access ON folioharbor.library_memberships USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY invitations_owner_access ON folioharbor.library_invitations USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY memberships_runtime_read ON folioharbor.library_memberships FOR SELECT USING (
  library_id=folioharbor.current_library_id() AND status='active' AND
  (folioharbor.is_worker() OR user_id=folioharbor.current_user_id()));
CREATE POLICY libraries_runtime_read ON folioharbor.libraries FOR SELECT USING (
  library_id=folioharbor.current_library_id() AND (folioharbor.is_worker() OR EXISTS(
    SELECT 1 FROM folioharbor.library_memberships m WHERE m.library_id=libraries.library_id
    AND m.user_id=folioharbor.current_user_id() AND m.status='active')));
CREATE POLICY invitations_runtime_read ON folioharbor.library_invitations FOR SELECT USING (
  library_id=folioharbor.current_library_id() AND (folioharbor.is_worker() OR EXISTS(
    SELECT 1 FROM folioharbor.library_memberships m WHERE m.library_id=library_invitations.library_id
    AND m.user_id=folioharbor.current_user_id() AND m.status='active')));

GRANT SELECT ON folioharbor.roles,folioharbor.permissions,folioharbor.role_permissions TO folioharbor_api,folioharbor_worker;
GRANT SELECT ON folioharbor.libraries,folioharbor.library_memberships,folioharbor.library_invitations TO folioharbor_api,folioharbor_worker;

CREATE TABLE folioharbor.audit_events (
 audit_event_id uuid NOT NULL, actor_id uuid, effective_actor_id uuid, library_id uuid NOT NULL,
 action_code text NOT NULL, resource_type text NOT NULL CHECK(resource_type IN('library','membership','invitation')),
 resource_id uuid NOT NULL, decision text NOT NULL CHECK(decision IN('allowed','denied')), reason_code text,
 request_id text NOT NULL, source text NOT NULL CHECK(source IN('api','worker')), occurred_at timestamptz NOT NULL,
 network_hmac bytea CHECK(network_hmac IS NULL OR octet_length(network_hmac)=32),
 PRIMARY KEY(occurred_at,audit_event_id)
) PARTITION BY RANGE(occurred_at);
CREATE TABLE folioharbor.audit_events_default PARTITION OF folioharbor.audit_events DEFAULT;
ALTER TABLE folioharbor.audit_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.audit_events FORCE ROW LEVEL SECURITY;
CREATE POLICY audit_owner_access ON folioharbor.audit_events USING(current_user='folioharbor_owner') WITH CHECK(current_user='folioharbor_owner');
CREATE POLICY audit_runtime_insert ON folioharbor.audit_events FOR INSERT WITH CHECK(
 library_id=folioharbor.current_library_id() AND request_id=folioharbor.current_request_id()
 AND ((source='worker')=folioharbor.is_worker())
 AND (folioharbor.is_worker() OR actor_id=folioharbor.current_user_id()));
CREATE POLICY audit_runtime_read ON folioharbor.audit_events FOR SELECT USING(
 library_id=folioharbor.current_library_id() AND (folioharbor.is_worker() OR actor_id=folioharbor.current_user_id()));
REVOKE ALL ON folioharbor.audit_events,folioharbor.audit_events_default FROM PUBLIC;
GRANT SELECT,INSERT ON folioharbor.audit_events TO folioharbor_api,folioharbor_worker;

CREATE FUNCTION folioharbor.audit_record_denial(
 p_id uuid,p_actor uuid,p_effective uuid,p_library uuid,p_action text,p_type text,p_resource uuid,
 p_reason text,p_request text,p_source text,p_at timestamptz,p_network bytea
) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF p_library IS DISTINCT FROM folioharbor.current_library_id()
 OR p_request IS DISTINCT FROM folioharbor.current_request_id()
 OR (p_source='worker') IS DISTINCT FROM folioharbor.is_worker()
 OR (NOT folioharbor.is_worker() AND p_actor IS DISTINCT FROM folioharbor.current_user_id())
 OR p_type NOT IN('library','membership','invitation') OR p_reason IS NULL THEN
  RAISE EXCEPTION 'invalid audit denial facts' USING ERRCODE='22023';
 END IF;
 INSERT INTO folioharbor.audit_events VALUES
 (p_id,p_actor,p_effective,p_library,p_action,p_type,p_resource,'denied',p_reason,p_request,p_source,p_at,p_network);
END $$;
ALTER FUNCTION folioharbor.audit_record_denial(uuid,uuid,uuid,uuid,text,text,uuid,text,text,text,timestamptz,bytea) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.audit_record_denial(uuid,uuid,uuid,uuid,text,text,uuid,text,text,text,timestamptz,bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.audit_record_denial(uuid,uuid,uuid,uuid,text,text,uuid,text,text,text,timestamptz,bytea) TO folioharbor_api,folioharbor_worker;

REVOKE EXECUTE ON FUNCTION folioharbor.library_create_invitation(uuid,uuid,uuid,text,text,text,bytea,timestamptz,timestamptz) FROM folioharbor_api;
REVOKE EXECUTE ON FUNCTION folioharbor.library_change_role(uuid,uuid,uuid,text,timestamptz) FROM folioharbor_api;
REVOKE EXECUTE ON FUNCTION folioharbor.library_remove_member(uuid,uuid,uuid,timestamptz) FROM folioharbor_api;
REVOKE EXECUTE ON FUNCTION folioharbor.library_update_settings(uuid,uuid,text,timestamptz) FROM folioharbor_api;

CREATE FUNCTION folioharbor.library_revalidate_grant(p_actor uuid,p_library uuid,p_permission text,p_version bigint)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE found_role text;
BEGIN
 SELECT role_code INTO found_role FROM folioharbor.library_memberships
 WHERE library_id=p_library AND user_id=p_actor AND status='active' AND version=p_version FOR UPDATE;
 RETURN found_role IS NOT NULL AND EXISTS(SELECT 1 FROM folioharbor.role_permissions WHERE role_code=found_role AND permission_code=p_permission);
END $$;

CREATE FUNCTION folioharbor.audit_record_allowed(
 p_id uuid,p_actor uuid,p_effective uuid,p_library uuid,p_action text,p_type text,p_resource uuid,
 p_decision text,p_reason text,p_request text,p_source text,p_at timestamptz,p_network bytea,
 p_expected_action text,p_expected_type text,p_expected_resource uuid
) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF p_actor IS NULL OR p_actor IS DISTINCT FROM p_effective
 OR p_library IS DISTINCT FROM folioharbor.current_library_id()
 OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
 OR p_request IS DISTINCT FROM folioharbor.current_request_id()
 OR p_action IS DISTINCT FROM p_expected_action OR p_type IS DISTINCT FROM p_expected_type
 OR p_resource IS DISTINCT FROM p_expected_resource OR p_decision IS DISTINCT FROM 'allowed'
 OR p_reason IS NOT NULL OR p_source IS DISTINCT FROM 'api' OR folioharbor.is_worker() THEN
  RAISE EXCEPTION 'audit event does not match grant' USING ERRCODE='22023';
 END IF;
 INSERT INTO folioharbor.audit_events VALUES
 (p_id,p_actor,p_effective,p_library,p_action,p_type,p_resource,p_decision,p_reason,p_request,p_source,p_at,p_network);
END $$;

CREATE FUNCTION folioharbor.library_create_invitation_authorized(
 p_invitation uuid,p_library uuid,p_actor uuid,p_email text,p_display text,p_role text,p_hash bytea,
 p_created timestamptz,p_expires timestamptz,p_version bigint,p_audit uuid,p_effective uuid,
 p_action text,p_type text,p_resource uuid,p_decision text,p_reason text,p_request text,p_source text,
 p_at timestamptz,p_network bytea
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE outcome text;
BEGIN
 IF NOT folioharbor.library_revalidate_grant(p_actor,p_library,'member.invite',p_version) THEN RETURN 'forbidden'; END IF;
 outcome:=folioharbor.library_create_invitation(p_invitation,p_library,p_actor,p_email,p_display,p_role,p_hash,p_created,p_expires);
 IF outcome='applied' THEN
  PERFORM folioharbor.audit_record_allowed(p_audit,p_actor,p_effective,p_library,p_action,p_type,p_resource,p_decision,p_reason,p_request,p_source,p_at,p_network,'member.invite','invitation',p_invitation);
 END IF;
 RETURN outcome;
END $$;

REVOKE ALL ON FUNCTION folioharbor.library_revalidate_grant(uuid,uuid,text,bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.audit_record_allowed(uuid,uuid,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea,text,text,uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.library_create_invitation_authorized(uuid,uuid,uuid,text,text,text,bytea,timestamptz,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.library_create_invitation_authorized(uuid,uuid,uuid,text,text,text,bytea,timestamptz,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea) TO folioharbor_api;

CREATE FUNCTION folioharbor.library_change_role_authorized(
 p_actor uuid,p_library uuid,p_target uuid,p_role text,p_now timestamptz,p_version bigint,p_audit uuid,
 p_effective uuid,p_action text,p_type text,p_resource uuid,p_decision text,p_reason text,p_request text,
 p_source text,p_at timestamptz,p_network bytea
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE outcome text; BEGIN
 IF NOT folioharbor.library_revalidate_grant(p_actor,p_library,'library.manage',p_version) THEN RETURN 'forbidden'; END IF;
 outcome:=folioharbor.library_change_role(p_actor,p_library,p_target,p_role,p_now);
 IF outcome='applied' THEN PERFORM folioharbor.audit_record_allowed(p_audit,p_actor,p_effective,p_library,p_action,p_type,p_resource,p_decision,p_reason,p_request,p_source,p_at,p_network,'member.role.change','membership',p_target); END IF;
 RETURN outcome; END $$;
CREATE FUNCTION folioharbor.library_remove_member_authorized(
 p_actor uuid,p_library uuid,p_target uuid,p_now timestamptz,p_version bigint,p_audit uuid,p_effective uuid,
 p_action text,p_type text,p_resource uuid,p_decision text,p_reason text,p_request text,p_source text,p_at timestamptz,p_network bytea
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE outcome text; BEGIN
 IF NOT folioharbor.library_revalidate_grant(p_actor,p_library,'library.manage',p_version) THEN RETURN 'forbidden'; END IF;
 outcome:=folioharbor.library_remove_member(p_actor,p_library,p_target,p_now);
 IF outcome='applied' THEN PERFORM folioharbor.audit_record_allowed(p_audit,p_actor,p_effective,p_library,p_action,p_type,p_resource,p_decision,p_reason,p_request,p_source,p_at,p_network,'member.remove','membership',p_target); END IF;
 RETURN outcome; END $$;
CREATE FUNCTION folioharbor.library_update_settings_authorized(
 p_actor uuid,p_library uuid,p_name text,p_now timestamptz,p_version bigint,p_audit uuid,p_effective uuid,
 p_action text,p_type text,p_resource uuid,p_decision text,p_reason text,p_request text,p_source text,p_at timestamptz,p_network bytea
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE outcome text; BEGIN
 IF NOT folioharbor.library_revalidate_grant(p_actor,p_library,'library.manage',p_version) THEN RETURN 'forbidden'; END IF;
 outcome:=folioharbor.library_update_settings(p_actor,p_library,p_name,p_now);
 IF outcome='applied' THEN PERFORM folioharbor.audit_record_allowed(p_audit,p_actor,p_effective,p_library,p_action,p_type,p_resource,p_decision,p_reason,p_request,p_source,p_at,p_network,'library.manage','library',p_library); END IF;
 RETURN outcome; END $$;
REVOKE ALL ON FUNCTION folioharbor.library_change_role_authorized(uuid,uuid,uuid,text,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.library_remove_member_authorized(uuid,uuid,uuid,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.library_update_settings_authorized(uuid,uuid,text,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.library_change_role_authorized(uuid,uuid,uuid,text,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea),
 folioharbor.library_remove_member_authorized(uuid,uuid,uuid,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea),
 folioharbor.library_update_settings_authorized(uuid,uuid,text,timestamptz,bigint,uuid,uuid,text,text,uuid,text,text,text,text,timestamptz,bytea) TO folioharbor_api;

CREATE FUNCTION folioharbor.library_list_visible(p_actor uuid)
RETURNS TABLE(library_id uuid,name text) LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
 SELECT l.library_id,l.name FROM folioharbor.libraries l JOIN folioharbor.library_memberships m USING(library_id)
 WHERE p_actor=folioharbor.current_user_id() AND m.user_id=p_actor AND m.status='active' ORDER BY l.library_id
$$;
CREATE FUNCTION folioharbor.library_get_visible(p_actor uuid,p_library uuid,p_version bigint)
RETURNS TABLE(library_id uuid,name text) LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
 SELECT l.library_id,l.name FROM folioharbor.libraries l WHERE l.library_id=p_library AND EXISTS(
 SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
 WHERE m.library_id=p_library AND m.user_id=p_actor AND m.status='active' AND m.version=p_version AND p.permission_code='holding.view')
$$;
CREATE FUNCTION folioharbor.library_members_visible(p_actor uuid,p_library uuid,p_version bigint)
RETURNS TABLE(user_id uuid,role_code text) LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
 SELECT members.user_id,members.role_code FROM folioharbor.library_memberships members WHERE members.library_id=p_library AND members.status='active'
 AND EXISTS(SELECT 1 FROM folioharbor.library_memberships actor JOIN folioharbor.role_permissions p USING(role_code)
 WHERE actor.library_id=p_library AND actor.user_id=p_actor AND actor.status='active' AND actor.version=p_version AND p.permission_code='holding.view') ORDER BY members.user_id
$$;
REVOKE ALL ON FUNCTION folioharbor.library_list_visible(uuid),folioharbor.library_get_visible(uuid,uuid,bigint),folioharbor.library_members_visible(uuid,uuid,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.library_list_visible(uuid),folioharbor.library_get_visible(uuid,uuid,bigint),folioharbor.library_members_visible(uuid,uuid,bigint) TO folioharbor_api;

UPDATE folioharbor.schema_metadata SET schema_version=7,applied_at=clock_timestamp() WHERE singleton;
