import { useCallback, useEffect, useRef } from "react";
import type { TFunction } from "i18next";

import { ApiProblem, isAbortError, problemMessage } from "../../api/problem";

interface ErrorSummaryProps {
  message: string | null;
}

export function ErrorSummary({ message }: ErrorSummaryProps) {
  const summaryRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (message !== null) {
      summaryRef.current?.focus();
    }
  }, [message]);

  if (message === null) {
    return null;
  }
  return (
    <div ref={summaryRef} role="alert" tabIndex={-1}>
      {message}
    </div>
  );
}

interface FieldErrorProps {
  id: string;
  message: string | undefined;
}

export function FieldError({ id, message }: FieldErrorProps) {
  return message === undefined ? null : <p id={id}>{message}</p>;
}

export function requestErrorMessage(error: unknown, t: TFunction): string | null {
  if (isAbortError(error)) {
    return null;
  }
  if (error instanceof ApiProblem) {
    return problemMessage(t, error);
  }
  return t("problems.unknown", { requestId: "unknown" });
}

export function requestFieldErrors(error: unknown, t: TFunction): Readonly<Record<string, string>> {
  if (!(error instanceof ApiProblem) || error.problem.fields === undefined) {
    return {};
  }
  return Object.fromEntries(
    error.problem.fields.map(({ code, field }) => {
      if (code === "required") {
        if (field === "email") {
          return [field, t("validation.emailRequired")];
        }
        if (field === "password" || field === "new_password") {
          return [field, t("validation.passwordRequired")];
        }
        if (field === "token") {
          return [field, t("validation.tokenRequired")];
        }
      }
      return [field, t("problems.invalidRequest")];
    }),
  );
}

export function useRequestController(): () => AbortSignal {
  const activeController = useRef<AbortController | null>(null);

  useEffect(
    () => () => {
      activeController.current?.abort();
    },
    [],
  );

  return useCallback(() => {
    activeController.current?.abort();
    const controller = new AbortController();
    activeController.current = controller;
    return controller.signal;
  }, []);
}

export function inputDescription(error: string | undefined, id: string): string | undefined {
  return error === undefined ? undefined : id;
}

export function formText(data: FormData, field: string): string {
  const value = data.get(field);
  return typeof value === "string" ? value : "";
}
