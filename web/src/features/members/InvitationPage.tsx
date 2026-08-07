import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, useNavigate, useParams } from "react-router-dom";

import { requestErrorMessage, useRequestController } from "../auth/form";
import { useSession } from "../auth/session";
import { librariesQueryKey } from "../libraries/queries";
import { acceptInvitation, type InvitationAcceptance } from "./api";

function acceptanceMessage(result: InvitationAcceptance, t: ReturnType<typeof useTranslation>["t"]): string {
  if (result.status === "wrong_account") {
    return t("invitations.wrongAccount", { email: result.email_hint ?? "" });
  }
  return t(`invitations.${result.status}`);
}

export function InvitationPage() {
  const { t } = useTranslation();
  const { token = "" } = useParams();
  const session = useSession();
  const requestSignal = useRequestController();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const mutation = useMutation({
    mutationFn: () => acceptInvitation(token, requestSignal()),
    onSuccess: (result) => {
      if (result.status === "accepted" && result.library_id !== undefined && result.library_id !== null) {
        void queryClient.invalidateQueries({ queryKey: librariesQueryKey }).then(() => {
          void navigate(`/libraries/${encodeURIComponent(result.library_id ?? "")}/books`, { replace: true });
        });
      }
    },
  });

  if (session.status === "loading") {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (session.status === "error") {
    return <p role="alert">{requestErrorMessage(session.error, t)}</p>;
  }

  const returnTo = `/invitations/${encodeURIComponent(token)}`;
  return (
    <section aria-labelledby="invitation-title">
      <h2 id="invitation-title">{t("invitations.title")}</h2>
      {session.status === "anonymous" ? (
        <Link to={`/login?returnTo=${encodeURIComponent(returnTo)}`}>{t("invitations.login")}</Link>
      ) : (
        <>
          <button type="button" disabled={mutation.isPending} onClick={() => { mutation.mutate(); }}>
            {t("invitations.accept")}
          </button>
          {mutation.isPending ? <p role="status">{t("invitations.processing")}</p> : null}
          {mutation.isError ? <p role="alert">{requestErrorMessage(mutation.error, t)}</p> : null}
          {mutation.data !== undefined && mutation.data.status !== "accepted"
            ? <p role="status">{acceptanceMessage(mutation.data, t)}</p>
            : null}
        </>
      )}
    </section>
  );
}
