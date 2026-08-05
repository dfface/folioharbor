CREATE TABLE folioharbor.roles (
    role_code text PRIMARY KEY,
    display_name text NOT NULL,
    is_builtin boolean NOT NULL DEFAULT true CHECK (is_builtin)
);

CREATE TABLE folioharbor.permissions (
    permission_code text PRIMARY KEY
);

CREATE TABLE folioharbor.role_permissions (
    role_code text NOT NULL REFERENCES folioharbor.roles(role_code) ON DELETE RESTRICT,
    permission_code text NOT NULL REFERENCES folioharbor.permissions(permission_code) ON DELETE RESTRICT,
    PRIMARY KEY (role_code, permission_code)
);

CREATE TABLE folioharbor.libraries (
    library_id uuid PRIMARY KEY,
    personal_owner_id uuid REFERENCES folioharbor.user_accounts(user_id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (length(btrim(name)) > 0),
    quota_used_bytes bigint NOT NULL DEFAULT 0 CHECK (quota_used_bytes >= 0),
    quota_reserved_bytes bigint NOT NULL DEFAULT 0 CHECK (quota_reserved_bytes >= 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0)
);

CREATE UNIQUE INDEX libraries_one_personal_per_owner
    ON folioharbor.libraries(personal_owner_id) WHERE personal_owner_id IS NOT NULL;

CREATE TABLE folioharbor.library_memberships (
    library_id uuid NOT NULL REFERENCES folioharbor.libraries(library_id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id) ON DELETE RESTRICT,
    role_code text NOT NULL REFERENCES folioharbor.roles(role_code) ON DELETE RESTRICT,
    status text NOT NULL CHECK (status IN ('active', 'removed')),
    joined_at timestamptz NOT NULL,
    removed_at timestamptz,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    CHECK ((status = 'active') = (removed_at IS NULL))
);

CREATE UNIQUE INDEX library_memberships_one_active
    ON folioharbor.library_memberships(library_id, user_id) WHERE status = 'active';
CREATE INDEX library_memberships_user_active
    ON folioharbor.library_memberships(user_id, library_id) WHERE status = 'active';

CREATE TABLE folioharbor.library_invitations (
    invitation_id uuid PRIMARY KEY,
    library_id uuid NOT NULL REFERENCES folioharbor.libraries(library_id) ON DELETE CASCADE,
    invited_by uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id) ON DELETE RESTRICT,
    normalized_email text NOT NULL,
    display_email text NOT NULL,
    role_code text NOT NULL REFERENCES folioharbor.roles(role_code) ON DELETE RESTRICT,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    consumed_by uuid REFERENCES folioharbor.user_accounts(user_id) ON DELETE RESTRICT,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    CHECK (expires_at > created_at),
    CHECK ((consumed_at IS NULL) = (consumed_by IS NULL))
);

CREATE FUNCTION folioharbor.library_provision_personal(p_library_id uuid, p_user_id uuid, p_now timestamptz)
RETURNS TABLE(library_id uuid, name text) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
#variable_conflict use_column
DECLARE found_id uuid; found_name text;
BEGIN
    INSERT INTO folioharbor.libraries(library_id, personal_owner_id, name, created_at, updated_at)
    VALUES (p_library_id, p_user_id, 'Personal Library', p_now, p_now)
    ON CONFLICT (personal_owner_id) WHERE personal_owner_id IS NOT NULL DO NOTHING;
    SELECT l.library_id, l.name INTO found_id, found_name FROM folioharbor.libraries l WHERE l.personal_owner_id = p_user_id;
    INSERT INTO folioharbor.library_memberships(library_id, user_id, role_code, status, joined_at)
    VALUES (found_id, p_user_id, 'owner', 'active', p_now)
    ON CONFLICT (library_id, user_id) WHERE status = 'active' DO NOTHING;
    RETURN QUERY SELECT found_id, found_name;
END $$;

CREATE FUNCTION folioharbor.library_create_invitation(
    p_invitation_id uuid, p_library_id uuid, p_invited_by uuid, p_email text, p_display_email text,
    p_role text, p_hash bytea, p_created_at timestamptz, p_expires_at timestamptz
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
    PERFORM 1 FROM folioharbor.libraries WHERE library_id = p_library_id FOR UPDATE;
    IF NOT EXISTS (SELECT 1 FROM folioharbor.library_memberships WHERE library_id=p_library_id AND user_id=p_invited_by AND role_code='owner' AND status='active') THEN RETURN 'forbidden'; END IF;
    INSERT INTO folioharbor.library_invitations VALUES
      (p_invitation_id,p_library_id,p_invited_by,p_email,p_display_email,p_role,p_hash,p_created_at,p_expires_at,NULL,NULL,1);
    RETURN 'applied';
END $$;

CREATE FUNCTION folioharbor.library_accept_invitation(p_user_id uuid, p_hash bytea, p_now timestamptz)
RETURNS TABLE(outcome text, accepted_library_id uuid) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
#variable_conflict use_column
DECLARE
    invitation folioharbor.library_invitations%ROWTYPE;
    authenticated_email text;
BEGIN
    SELECT i.* INTO invitation FROM folioharbor.library_invitations AS i
      WHERE i.token_hash=p_hash AND i.consumed_at IS NULL FOR UPDATE;
    SELECT a.normalized_email INTO authenticated_email
      FROM folioharbor.user_accounts AS a
      WHERE a.user_id=p_user_id
      FOR KEY SHARE;
    IF invitation.invitation_id IS NULL OR authenticated_email IS NULL OR invitation.expires_at <= p_now OR invitation.normalized_email <> authenticated_email THEN
      RETURN QUERY SELECT 'invalid'::text, NULL::uuid; RETURN;
    END IF;
    INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at)
      VALUES(invitation.library_id,p_user_id,invitation.role_code,'active',p_now)
      ON CONFLICT (library_id,user_id) WHERE status='active' DO NOTHING;
    UPDATE folioharbor.library_invitations AS i SET consumed_at=p_now,consumed_by=p_user_id,version=i.version+1 WHERE i.invitation_id=invitation.invitation_id;
    RETURN QUERY SELECT 'accepted'::text, invitation.library_id;
END $$;

CREATE FUNCTION folioharbor.library_change_role(p_actor uuid,p_library uuid,p_target uuid,p_role text,p_now timestamptz)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE old_role text;
BEGIN
  PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
  IF NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships WHERE library_id=p_library AND user_id=p_actor AND role_code='owner' AND status='active') THEN RETURN 'forbidden'; END IF;
  SELECT role_code INTO old_role FROM folioharbor.library_memberships WHERE library_id=p_library AND user_id=p_target AND status='active';
  IF old_role IS NULL THEN RETURN 'not_found'; END IF;
  IF old_role='owner' AND p_role<>'owner' AND (SELECT count(*) FROM folioharbor.library_memberships WHERE library_id=p_library AND role_code='owner' AND status='active') <= 1 THEN RETURN 'last_owner'; END IF;
  UPDATE folioharbor.library_memberships SET role_code=p_role,version=version+1 WHERE library_id=p_library AND user_id=p_target AND status='active'; RETURN 'applied';
END $$;

CREATE FUNCTION folioharbor.library_remove_member(p_actor uuid,p_library uuid,p_target uuid,p_now timestamptz)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE old_role text;
BEGIN
  PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
  IF NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships WHERE library_id=p_library AND user_id=p_actor AND role_code='owner' AND status='active') THEN RETURN 'forbidden'; END IF;
  SELECT role_code INTO old_role FROM folioharbor.library_memberships WHERE library_id=p_library AND user_id=p_target AND status='active';
  IF old_role IS NULL THEN RETURN 'not_found'; END IF;
  IF old_role='owner' AND (SELECT count(*) FROM folioharbor.library_memberships WHERE library_id=p_library AND role_code='owner' AND status='active') <= 1 THEN RETURN 'last_owner'; END IF;
  UPDATE folioharbor.library_memberships SET status='removed',removed_at=p_now,version=version+1 WHERE library_id=p_library AND user_id=p_target AND status='active'; RETURN 'applied';
END $$;

CREATE FUNCTION folioharbor.library_update_settings(p_actor uuid,p_library uuid,p_name text,p_now timestamptz)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
  PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
  IF NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships WHERE library_id=p_library AND user_id=p_actor AND role_code='owner' AND status='active') THEN RETURN 'forbidden'; END IF;
  UPDATE folioharbor.libraries SET name=p_name,updated_at=p_now,version=version+1 WHERE library_id=p_library; RETURN 'applied';
END $$;

REVOKE ALL ON FUNCTION folioharbor.library_provision_personal(uuid,uuid,timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.library_create_invitation(uuid,uuid,uuid,text,text,text,bytea,timestamptz,timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.library_accept_invitation(uuid,bytea,timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.library_change_role(uuid,uuid,uuid,text,timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.library_remove_member(uuid,uuid,uuid,timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.library_update_settings(uuid,uuid,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.library_provision_personal(uuid,uuid,timestamptz),folioharbor.library_create_invitation(uuid,uuid,uuid,text,text,text,bytea,timestamptz,timestamptz),folioharbor.library_accept_invitation(uuid,bytea,timestamptz),folioharbor.library_change_role(uuid,uuid,uuid,text,timestamptz),folioharbor.library_remove_member(uuid,uuid,uuid,timestamptz),folioharbor.library_update_settings(uuid,uuid,text,timestamptz) TO folioharbor_api;
