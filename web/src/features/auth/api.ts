import { apiClient } from "../../api/client";
import type { components } from "../../api/generated";

export type RegisterRequest = components["schemas"]["RegisterRequest"];
export type VerifyEmailRequest = components["schemas"]["VerifyEmailRequest"];
export type LoginRequest = components["schemas"]["LoginRequest"];
export type ForgotPasswordRequest = components["schemas"]["ForgotPasswordRequest"];
export type ResetPasswordRequest = components["schemas"]["ResetPasswordRequest"];
export type AcceptedResponse = components["schemas"]["AcceptedResponse"];
export type SessionIssued = components["schemas"]["SessionIssuedResponse"];
export type Session = components["schemas"]["Session"];

function signalOption(signal: AbortSignal | undefined): { signal?: AbortSignal } {
  return signal === undefined ? {} : { signal };
}

export function register(input: RegisterRequest, signal?: AbortSignal): Promise<AcceptedResponse> {
  return apiClient.request("/api/v1/auth/register", {
    body: input,
    method: "POST",
    ...signalOption(signal),
  });
}

export function verifyEmail(input: VerifyEmailRequest, signal?: AbortSignal): Promise<void> {
  return apiClient.request("/api/v1/auth/verify-email", {
    body: input,
    method: "POST",
    ...signalOption(signal),
  });
}

export function login(input: LoginRequest, signal?: AbortSignal): Promise<SessionIssued> {
  return apiClient.request("/api/v1/auth/login", {
    body: input,
    method: "POST",
    ...signalOption(signal),
  });
}

export function logout(signal?: AbortSignal): Promise<void> {
  return apiClient.request("/api/v1/auth/logout", {
    method: "POST",
    ...signalOption(signal),
  });
}

export function forgotPassword(input: ForgotPasswordRequest, signal?: AbortSignal): Promise<AcceptedResponse> {
  return apiClient.request("/api/v1/auth/forgot-password", {
    body: input,
    method: "POST",
    ...signalOption(signal),
  });
}

export function resetPassword(input: ResetPasswordRequest, signal?: AbortSignal): Promise<SessionIssued> {
  return apiClient.request("/api/v1/auth/reset-password", {
    body: input,
    method: "POST",
    ...signalOption(signal),
  });
}

export function currentSession(signal?: AbortSignal): Promise<Session> {
  return apiClient.request("/api/v1/auth/session", signalOption(signal));
}

export function listSessions(signal?: AbortSignal): Promise<Session[]> {
  return apiClient.request("/api/v1/auth/sessions", signalOption(signal));
}

export function revokeSession(sessionId: string, signal?: AbortSignal): Promise<void> {
  return apiClient.request(`/api/v1/auth/sessions/${encodeURIComponent(sessionId)}/revoke`, {
    method: "POST",
    ...signalOption(signal),
  });
}

export async function revokeAllSessions(sessions: readonly Session[], signal?: AbortSignal): Promise<void> {
  const current = sessions.find((session) => session.is_current);
  for (const session of sessions) {
    if (!session.is_current) {
      await revokeSession(session.session_id, signal);
    }
  }
  if (current !== undefined) {
    await revokeSession(current.session_id, signal);
  }
}
