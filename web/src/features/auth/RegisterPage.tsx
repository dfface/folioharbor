import { useState, type SyntheticEvent } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";

import { register, type RegisterRequest } from "./api";
import {
  ErrorSummary,
  FieldError,
  formText,
  inputDescription,
  requestErrorMessage,
  requestFieldErrors,
  useRequestController,
} from "./form";

export function RegisterPage() {
  const { t } = useTranslation();
  const requestSignal = useRequestController();
  const [error, setError] = useState<string | null>(null);
  const [fieldErrors, setFieldErrors] = useState<Readonly<Record<string, string>>>({});
  const [status, setStatus] = useState<string | null>(null);
  const mutation = useMutation({
    mutationFn: (input: RegisterRequest) => register(input, requestSignal()),
    onError: (cause) => {
      setStatus(null);
      setFieldErrors(requestFieldErrors(cause, t));
      setError(requestErrorMessage(cause, t));
    },
    onSuccess: () => {
      setError(null);
      setFieldErrors({});
      setStatus(t("auth.verificationRequired"));
    },
  });

  function handleSubmit(event: SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    setStatus(null);
    const data = new FormData(event.currentTarget);
    const email = formText(data, "email").trim();
    const password = formText(data, "password");
    const errors: Record<string, string> = {};
    if (email.length === 0) {
      errors.email = t("validation.emailRequired");
    }
    if (password.length === 0) {
      errors.password = t("validation.passwordRequired");
    }
    setFieldErrors(errors);
    if (Object.keys(errors).length > 0) {
      setError(t("validation.summary"));
      return;
    }
    setError(null);
    mutation.mutate({ email, password });
  }

  return (
    <section aria-labelledby="register-title">
      <h2 id="register-title">{t("auth.register")}</h2>
      <ErrorSummary message={error} />
      {status === null ? null : <p role="status" aria-live="polite">{status}</p>}
      {mutation.isPending ? <p role="status" aria-live="polite">{t("auth.submitting")}</p> : null}
      <form noValidate onSubmit={handleSubmit}>
        <div>
          <label htmlFor="register-email">{t("auth.email")}</label>
          <input
            id="register-email"
            name="email"
            type="email"
            autoComplete="email"
            aria-describedby={inputDescription(fieldErrors.email, "register-email-error")}
            aria-invalid={fieldErrors.email === undefined ? undefined : true}
          />
          <FieldError id="register-email-error" message={fieldErrors.email} />
        </div>
        <div>
          <label htmlFor="register-password">{t("auth.password")}</label>
          <input
            id="register-password"
            name="password"
            type="password"
            autoComplete="new-password"
            aria-describedby={inputDescription(fieldErrors.password, "register-password-error")}
            aria-invalid={fieldErrors.password === undefined ? undefined : true}
          />
          <FieldError id="register-password-error" message={fieldErrors.password} />
        </div>
        <button type="submit" disabled={mutation.isPending}>{t("auth.register")}</button>
      </form>
      <p><Link to="/verify-email">{t("auth.verifyEmail")}</Link></p>
      <p>{t("auth.haveAccount")} <Link to="/login">{t("auth.login")}</Link></p>
    </section>
  );
}
