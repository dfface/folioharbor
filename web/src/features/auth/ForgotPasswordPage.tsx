import { useState, type SyntheticEvent } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { forgotPassword, type ForgotPasswordRequest } from "./api";
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

export function ForgotPasswordPage() {
  const { t } = useTranslation();
  const requestSignal = useRequestController();
  const { error, focusVersion, setError } = useErrorSummaryState();
  const [fieldError, setFieldError] = useState<string | undefined>();
  const [status, setStatus] = useState<string | null>(null);
  const mutation = useMutation({
    mutationFn: (input: ForgotPasswordRequest) => forgotPassword(input, requestSignal()),
    onError: (cause) => {
      setStatus(null);
      setFieldError(requestFieldErrors(cause, t).email);
      setError(requestErrorMessage(cause, t));
    },
    onSuccess: () => {
      setError(null);
      setFieldError(undefined);
      setStatus(t("auth.resetRequested"));
    },
  });

  function handleSubmit(event: SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    setStatus(null);
    const email = formText(new FormData(event.currentTarget), "email").trim();
    if (email.length === 0) {
      setFieldError(t("validation.emailRequired"));
      setError(t("validation.summary"));
      return;
    }
    setFieldError(undefined);
    setError(null);
    mutation.mutate({ email });
  }

  return (
    <section aria-labelledby="forgot-title">
      <h2 id="forgot-title">{t("auth.forgotPassword")}</h2>
      <ErrorSummary focusVersion={focusVersion} message={error} />
      {status === null ? null : <p role="status" aria-live="polite">{status}</p>}
      {mutation.isPending ? <p role="status" aria-live="polite">{t("auth.submitting")}</p> : null}
      <form noValidate onSubmit={handleSubmit}>
        <label htmlFor="forgot-email">{t("auth.email")}</label>
        <input
          id="forgot-email"
          name="email"
          type="email"
          autoComplete="email"
          aria-describedby={inputDescription(fieldError, "forgot-email-error")}
          aria-invalid={fieldError === undefined ? undefined : true}
        />
        <FieldError id="forgot-email-error" message={fieldError} />
        <button type="submit" disabled={mutation.isPending}>{t("auth.sendReset")}</button>
      </form>
      <p><Link to="/reset-password">{t("auth.resetPassword")}</Link></p>
      <p><Link to="/login">{t("auth.login")}</Link></p>
    </section>
  );
}
