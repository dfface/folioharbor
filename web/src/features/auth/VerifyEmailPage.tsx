import { useState, type SyntheticEvent } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, useSearchParams } from "react-router-dom";

import { verifyEmail, type VerifyEmailRequest } from "./api";
import {
  ErrorSummary,
  FieldError,
  formText,
  inputDescription,
  requestErrorMessage,
  requestFieldErrors,
  useErrorSummaryState,
  useRequestController,
} from "./form";

export function VerifyEmailPage() {
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const requestSignal = useRequestController();
  const { error, focusVersion, setError } = useErrorSummaryState();
  const [fieldError, setFieldError] = useState<string | undefined>();
  const [status, setStatus] = useState<string | null>(null);
  const mutation = useMutation({
    mutationFn: (input: VerifyEmailRequest) => verifyEmail(input, requestSignal()),
    onError: (cause) => {
      setStatus(null);
      setFieldError(requestFieldErrors(cause, t).token);
      setError(requestErrorMessage(cause, t));
    },
    onSuccess: () => {
      setError(null);
      setFieldError(undefined);
      setStatus(t("auth.emailVerified"));
    },
  });

  function handleSubmit(event: SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    setStatus(null);
    const token = formText(new FormData(event.currentTarget), "token").trim();
    if (token.length === 0) {
      setFieldError(t("validation.tokenRequired"));
      setError(t("validation.summary"));
      return;
    }
    setFieldError(undefined);
    setError(null);
    mutation.mutate({ token });
  }

  return (
    <section aria-labelledby="verify-title">
      <h2 id="verify-title">{t("auth.verifyEmail")}</h2>
      <ErrorSummary focusVersion={focusVersion} message={error} />
      {status === null ? null : <p role="status" aria-live="polite">{status}</p>}
      {mutation.isPending ? <p role="status" aria-live="polite">{t("auth.submitting")}</p> : null}
      <form noValidate onSubmit={handleSubmit}>
        <label htmlFor="verification-token">{t("auth.verificationToken")}</label>
        <input
          id="verification-token"
          name="token"
          defaultValue={searchParams.get("token") ?? ""}
          autoComplete="one-time-code"
          aria-describedby={inputDescription(fieldError, "verification-token-error")}
          aria-invalid={fieldError === undefined ? undefined : true}
        />
        <FieldError id="verification-token-error" message={fieldError} />
        <button type="submit" disabled={mutation.isPending}>{t("auth.verifyEmail")}</button>
      </form>
      <p><Link to="/login">{t("auth.login")}</Link></p>
    </section>
  );
}
