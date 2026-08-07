import { useState, type SyntheticEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Navigate } from "react-router";

import { formText, requestErrorMessage, useRequestController } from "../auth/form";
import { useCurrentLibrary } from "../libraries/LibraryLayout";
import {
  changeMemberRole,
  inviteMember,
  removeMember,
  type InvitationRole,
  type InviteMemberRequest,
  type MemberRole,
} from "./api";
import { membersQueryKey, useMembers } from "./queries";

type Notice = "invited" | "removed" | "roleUpdated" | null;

export function MembersPage() {
  const { t } = useTranslation();
  const library = useCurrentLibrary();
  const canInviteMembers = library.capabilities.can_invite_members;
  const canManageMembers = library.capabilities.can_manage_members;
  const members = useMembers(library.library_id, canManageMembers);
  const queryClient = useQueryClient();
  const requestSignal = useRequestController();
  const [notice, setNotice] = useState<Notice>(null);
  const [error, setError] = useState<unknown>(null);

  const refresh = () => queryClient.invalidateQueries({ queryKey: membersQueryKey(library.library_id) });
  const invitation = useMutation({
    mutationFn: (input: InviteMemberRequest) => inviteMember(library.library_id, input, requestSignal()),
    onError: (cause) => { setNotice(null); setError(cause); },
    onSuccess: () => { setError(null); setNotice("invited"); },
  });
  const roleChange = useMutation({
    mutationFn: ({ role, userId }: { role: MemberRole; userId: string }) =>
      changeMemberRole(library.library_id, userId, role, requestSignal()),
    onError: (cause) => { setNotice(null); setError(cause); },
    onSuccess: () => { setError(null); setNotice("roleUpdated"); void refresh(); },
  });
  const removal = useMutation({
    mutationFn: (userId: string) => removeMember(library.library_id, userId, requestSignal()),
    onError: (cause) => { setNotice(null); setError(cause); },
    onSuccess: () => { setError(null); setNotice("removed"); void refresh(); },
  });

  if (!canInviteMembers && !canManageMembers) {
    return <Navigate to={`/libraries/${encodeURIComponent(library.library_id)}/books`} replace />;
  }

  function submitInvitation(event: SyntheticEvent<HTMLFormElement>) {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    setError(null);
    setNotice(null);
    invitation.mutate({
      email: formText(data, "email").trim(),
      role: (formText(data, "role") || "reader") as InvitationRole,
    });
  }

  return (
    <section aria-labelledby="members-title">
      <h3 id="members-title">{t("members.title")}</h3>
      {error === null ? null : <p role="alert">{requestErrorMessage(error, t)}</p>}
      {notice === null ? null : <p role="status">{t(`members.${notice}`)}</p>}
      {canInviteMembers ? (
        <form onSubmit={submitInvitation}>
          <label htmlFor="invite-email">{t("members.inviteEmail")}</label>
          <input id="invite-email" name="email" type="email" required />
          <label htmlFor="invite-role">{t("members.inviteRole")}</label>
          <select id="invite-role" name="role" defaultValue="reader">
            <option value="reader">{t("roles.reader")}</option>
            <option value="editor">{t("roles.editor")}</option>
          </select>
          <button type="submit" disabled={invitation.isPending}>{t("members.send")}</button>
        </form>
      ) : null}
      {canManageMembers && members.isPending ? <p role="status">{t("members.loading")}</p> : null}
      {canManageMembers && members.isError ? <p role="alert">{requestErrorMessage(members.error, t)}</p> : null}
      {!canManageMembers || members.data === undefined ? null : (
        <ul>
          {members.data.map((member) => (
            <li key={member.user_id}>
              <span>{member.user_id}</span>{" "}
              <label>
                <span className="visually-hidden">{t("members.roleFor", { userId: member.user_id })}</span>
                <select
                  aria-label={t("members.roleFor", { userId: member.user_id })}
                  value={member.role}
                  disabled={roleChange.isPending || removal.isPending}
                  onChange={(event) => {
                    roleChange.mutate({ role: event.currentTarget.value as MemberRole, userId: member.user_id });
                  }}
                >
                  <option value="reader">{t("roles.reader")}</option>
                  <option value="editor">{t("roles.editor")}</option>
                  <option value="owner">{t("roles.owner")}</option>
                </select>
              </label>{" "}
              <button
                type="button"
                disabled={roleChange.isPending || removal.isPending}
                onClick={() => { removal.mutate(member.user_id); }}
              >
                {t("members.remove", { userId: member.user_id })}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
