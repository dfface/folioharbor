import type { TFunction } from "i18next";

import type { components } from "./generated";

export type ProblemDetails = components["schemas"]["ProblemDetails"];

export class ApiProblem extends Error {
  readonly problem: ProblemDetails;

  constructor(problem: ProblemDetails) {
    super(problem.code);
    this.name = "ApiProblem";
    this.problem = problem;
  }
}

const problemKeys: Readonly<Record<string, string>> = {
  csrf_failed: "problems.csrfFailed",
  email_verification_required: "problems.emailVerificationRequired",
  invalid_json_body: "problems.invalidRequest",
  invalid_or_expired_password_reset_token: "problems.invalidResetToken",
  invalid_or_expired_verification_token: "problems.invalidVerificationToken",
  invalid_password: "problems.invalidPassword",
  invalid_registration: "problems.invalidRegistration",
  library_requires_owner: "problems.libraryRequiresOwner",
  malformed_json: "problems.invalidRequest",
  payload_too_large: "uploads.tooLarge",
  rate_limited: "problems.rateLimited",
  session_not_found: "problems.sessionNotFound",
  unauthenticated: "problems.unauthenticated",
  unsupported_media_type: "problems.invalidRequest",
};

export function problemMessage(t: TFunction, error: ApiProblem): string {
  const key = problemKeys[error.problem.code];
  if (key !== undefined) {
    return t(key);
  }
  return t("problems.unknown", { requestId: error.problem.request_id });
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}
