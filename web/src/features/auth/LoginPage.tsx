import { useState, type SyntheticEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, useNavigate, useSearchParams } from "react-router";

import { login, type LoginRequest } from "./api";
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
import { resetAuthIdentityQueries } from "./session";

export function LoginPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const queryClient = useQueryClient();
  const requestSignal = useRequestController();
  const { error, focusVersion, setError } = useErrorSummaryState();
  const [fieldErrors, setFieldErrors] = useState<Readonly<Record<string, string>>>({});
  const mutation = useMutation({
    mutationFn: (input: LoginRequest) => login(input, requestSignal()),
    onError: (cause) => {
      setFieldErrors(requestFieldErrors(cause, t));
      setError(requestErrorMessage(cause, t));
    },
    onSuccess: () => {
      setError(null);
      setFieldErrors({});
      void resetAuthIdentityQueries(queryClient).then(() => {
        const requestedReturn = searchParams.get("returnTo");
        const returnTo = requestedReturn?.startsWith("/invitations/") === true && !requestedReturn.startsWith("//")
          ? requestedReturn
          : "/";
        void navigate(returnTo, { replace: true });
      });
    },
  });

  function handleSubmit(event: SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
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
    <section aria-labelledby="login-title">
      <h2 id="login-title">{t("auth.login")}</h2>
      <ErrorSummary focusVersion={focusVersion} message={error} />
      {mutation.isPending ? <p role="status" aria-live="polite">{t("auth.submitting")}</p> : null}
      <form noValidate onSubmit={handleSubmit}>
        <div>
          <label htmlFor="login-email">{t("auth.email")}</label>
          <input
            id="login-email"
            name="email"
            type="email"
            autoComplete="email"
            aria-describedby={inputDescription(fieldErrors.email, "login-email-error")}
            aria-invalid={fieldErrors.email === undefined ? undefined : true}
          />
          <FieldError id="login-email-error" message={fieldErrors.email} />
        </div>
        <div>
          <label htmlFor="login-password">{t("auth.password")}</label>
          <input
            id="login-password"
            name="password"
            type="password"
            autoComplete="current-password"
            aria-describedby={inputDescription(fieldErrors.password, "login-password-error")}
            aria-invalid={fieldErrors.password === undefined ? undefined : true}
          />
          <FieldError id="login-password-error" message={fieldErrors.password} />
        </div>
        <button type="submit" disabled={mutation.isPending}>{t("auth.login")}</button>
      </form>
      <p><Link to="/forgot-password">{t("auth.forgotPassword")}</Link></p>
      <p>{t("auth.needAccount")} <Link to="/register">{t("auth.register")}</Link></p>
    </section>
  );
}
