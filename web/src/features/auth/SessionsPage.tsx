import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import {
  listSessions,
  revokeAllSessions,
  revokeSession,
  type Session,
} from "./api";
import { requestErrorMessage, useRequestController } from "./form";
import { resetAuthIdentityQueries, sessionsQueryKey } from "./session";

export function SessionsPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const requestSignal = useRequestController();
  const sessions = useQuery({
    queryFn: ({ signal }) => listSessions(signal),
    queryKey: sessionsQueryKey,
  });
  const revokeOne = useMutation({
    mutationFn: (session: Session) => revokeSession(session.session_id, requestSignal()),
    onSuccess: (_data, revokedSession) =>
      revokedSession.is_current
        ? resetAuthIdentityQueries(queryClient)
        : queryClient.invalidateQueries({ queryKey: sessionsQueryKey }),
  });
  const revokeAll = useMutation({
    mutationFn: (activeSessions: readonly Session[]) => revokeAllSessions(activeSessions, requestSignal()),
    onSuccess: () => resetAuthIdentityQueries(queryClient),
  });

  const error = sessions.error ?? revokeOne.error ?? revokeAll.error;

  return (
    <section aria-labelledby="sessions-title">
      <h2 id="sessions-title">{t("sessions.title")}</h2>
      {sessions.isPending ? <p role="status" aria-live="polite">{t("app.loading")}</p> : null}
      {error === null ? null : <p role="alert">{requestErrorMessage(error, t)}</p>}
      {revokeOne.isSuccess ? <p role="status" aria-live="polite">{t("sessions.revokedOne")}</p> : null}
      {sessions.data?.length === 0 ? <p>{t("sessions.empty")}</p> : null}
      <ul>
        {sessions.data?.map((session) => (
          <li key={session.session_id}>
            <span>{session.session_id}</span>{" "}
            <span>{t(`sessions.${session.status}`)}</span>{" "}
            {session.is_current ? <strong>{t("sessions.current")}</strong> : null}{" "}
            {session.status === "revoked" ? null : (
              <button
                type="button"
                aria-label={t("sessions.revoke", { sessionId: session.session_id })}
                disabled={revokeOne.isPending || revokeAll.isPending}
                onClick={() => { revokeOne.mutate(session); }}
              >
                {t("sessions.revoke", { sessionId: session.session_id })}
              </button>
            )}
          </li>
        ))}
      </ul>
      <button
        type="button"
        disabled={sessions.data === undefined || sessions.data.length === 0 || revokeAll.isPending || revokeOne.isPending}
        onClick={() => {
          if (sessions.data !== undefined) {
            revokeAll.mutate(sessions.data);
          }
        }}
      >
        {t("sessions.revokeAll")}
      </button>
    </section>
  );
}
