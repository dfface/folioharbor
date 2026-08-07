import { useState, type SyntheticEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";

import { resetPassword, type ResetPasswordRequest } from "./api";
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

export function ResetPasswordPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const queryClient = useQueryClient();
  const requestSignal = useRequestController();
  const { error, focusVersion, setError } = useErrorSummaryState();
  const [fieldErrors, setFieldErrors] = useState<Readonly<Record<string, string>>>({});
  const mutation = useMutation({
    mutationFn: (input: ResetPasswordRequest) => resetPassword(input, requestSignal()),
    onError: (cause) => {
      setFieldErrors(requestFieldErrors(cause, t));
      setError(requestErrorMessage(cause, t));
    },
    onSuccess: () => {
      setError(null);
      setFieldErrors({});
      void resetAuthIdentityQueries(queryClient).then(() => {
        void navigate("/", { replace: true });
      });
    },
  });

  function handleSubmit(event: SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const token = formText(data, "token").trim();
    const newPassword = formText(data, "new_password");
    const errors: Record<string, string> = {};
    if (token.length === 0) {
      errors.token = t("validation.tokenRequired");
    }
    if (newPassword.length === 0) {
      errors.new_password = t("validation.passwordRequired");
    }
    setFieldErrors(errors);
    if (Object.keys(errors).length > 0) {
      setError(t("validation.summary"));
      return;
    }
    setError(null);
    mutation.mutate({ new_password: newPassword, token });
  }

  return (
    <section aria-labelledby="reset-title">
      <h2 id="reset-title">{t("auth.resetPassword")}</h2>
      <ErrorSummary focusVersion={focusVersion} message={error} />
      {mutation.isPending ? <p role="status" aria-live="polite">{t("auth.submitting")}</p> : null}
      <form noValidate onSubmit={handleSubmit}>
        <div>
          <label htmlFor="reset-token">{t("auth.resetToken")}</label>
          <input
            id="reset-token"
            name="token"
            defaultValue={searchParams.get("token") ?? ""}
            autoComplete="one-time-code"
            aria-describedby={inputDescription(fieldErrors.token, "reset-token-error")}
            aria-invalid={fieldErrors.token === undefined ? undefined : true}
          />
          <FieldError id="reset-token-error" message={fieldErrors.token} />
        </div>
        <div>
          <label htmlFor="reset-password">{t("auth.newPassword")}</label>
          <input
            id="reset-password"
            name="new_password"
            type="password"
            autoComplete="new-password"
            aria-describedby={inputDescription(fieldErrors.new_password, "reset-password-error")}
            aria-invalid={fieldErrors.new_password === undefined ? undefined : true}
          />
          <FieldError id="reset-password-error" message={fieldErrors.new_password} />
        </div>
        <button type="submit" disabled={mutation.isPending}>{t("auth.resetPassword")}</button>
      </form>
    </section>
  );
}
