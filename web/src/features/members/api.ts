import { apiClient } from "../../api/client";
import type { components } from "../../api/generated";

export type LibraryMember = components["schemas"]["LibraryMember"];
export type MemberRole = LibraryMember["role"];
export type InvitationAcceptance = components["schemas"]["InvitationAcceptance"];

function signalOption(signal: AbortSignal | undefined): { signal?: AbortSignal } {
  return signal === undefined ? {} : { signal };
}

export function listMembers(libraryId: string, signal?: AbortSignal): Promise<LibraryMember[]> {
  return apiClient.request(`/api/v1/libraries/${encodeURIComponent(libraryId)}/members`, signalOption(signal));
}

export function inviteMember(
  libraryId: string,
  input: { email: string; role: MemberRole },
  signal?: AbortSignal,
): Promise<void> {
  return apiClient.request(`/api/v1/libraries/${encodeURIComponent(libraryId)}/invitations`, {
    body: input,
    method: "POST",
    ...signalOption(signal),
  });
}

export function changeMemberRole(
  libraryId: string,
  userId: string,
  role: MemberRole,
  signal?: AbortSignal,
): Promise<void> {
  return apiClient.request(
    `/api/v1/libraries/${encodeURIComponent(libraryId)}/members/${encodeURIComponent(userId)}`,
    { body: { role }, method: "PATCH", ...signalOption(signal) },
  );
}

export function removeMember(libraryId: string, userId: string, signal?: AbortSignal): Promise<void> {
  return apiClient.request(
    `/api/v1/libraries/${encodeURIComponent(libraryId)}/members/${encodeURIComponent(userId)}`,
    { method: "DELETE", ...signalOption(signal) },
  );
}

export function acceptInvitation(token: string, signal?: AbortSignal): Promise<InvitationAcceptance> {
  return apiClient.request("/api/v1/invitations/accept", {
    body: { token },
    method: "POST",
    ...signalOption(signal),
  });
}
