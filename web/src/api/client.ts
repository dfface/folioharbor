import type { components } from "./generated";
import { ApiProblem, type ProblemDetails } from "./problem";

type Method = "DELETE" | "GET" | "HEAD" | "OPTIONS" | "PATCH" | "POST" | "PUT";

export interface ApiRequestOptions {
  body?: unknown;
  headers?: HeadersInit;
  keepalive?: boolean;
  method?: Method;
  signal?: AbortSignal;
}

type ProblemFieldViolation = components["schemas"]["ProblemFieldViolation"];

function csrfToken(): string | undefined {
  const prefix = "folioharbor_csrf=";
  const cookie = document.cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(prefix));
  return cookie === undefined ? undefined : decodeURIComponent(cookie.slice(prefix.length));
}

function isUnsafe(method: Method): boolean {
  return method !== "GET" && method !== "HEAD" && method !== "OPTIONS";
}

function isFieldViolation(value: unknown): value is ProblemFieldViolation {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  return "field" in value && typeof value.field === "string" && "code" in value && typeof value.code === "string";
}

function isProblemDetails(value: unknown): value is ProblemDetails {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  const requiredStrings = ["type", "title", "detail", "instance", "code", "request_id"] as const;
  const stringsAreValid = requiredStrings.every((key) => typeof record[key] === "string");
  if (!stringsAreValid || typeof record.status !== "number") {
    return false;
  }
  return record.fields === undefined ||
    (Array.isArray(record.fields) && record.fields.every(isFieldViolation));
}

async function decodeProblem(response: Response): Promise<ProblemDetails> {
  const requestId = response.headers.get("X-Request-ID") ?? "unknown";
  const contentType = response.headers.get("Content-Type") ?? "";
  if (contentType.toLowerCase().includes("json")) {
    try {
      const value: unknown = await response.json();
      if (isProblemDetails(value)) {
        return value;
      }
    } catch {
      // Fall through to a safe local problem; malformed server details are never rendered.
    }
  }
  return {
    type: "about:blank",
    title: "Request failed",
    status: response.status,
    detail: "The request could not be completed.",
    instance: "",
    code: "unknown_problem",
    request_id: requestId,
  };
}

function requestUrl(path: string): string {
  const origin = typeof window === "undefined" ? "http://localhost" : window.location.origin;
  return new URL(path, origin).toString();
}

export const apiClient = {
  async request<T>(path: string, options: ApiRequestOptions = {}): Promise<T> {
    const method = options.method ?? "GET";
    const headers = new Headers(options.headers);
    let body: BodyInit | undefined;

    if (options.body !== undefined) {
      headers.set("Content-Type", "application/json");
      body = JSON.stringify(options.body);
    }

    if (isUnsafe(method)) {
      const token = csrfToken();
      if (token !== undefined) {
        headers.set("X-CSRF-Token", token);
      }
    }

    const init: RequestInit = { credentials: "include", headers, method };
    if (body !== undefined) {
      init.body = body;
    }
    if (options.signal !== undefined) {
      init.signal = options.signal;
    }
    if (options.keepalive !== undefined) {
      init.keepalive = options.keepalive;
    }

    const response = await fetch(requestUrl(path), init);
    if (!response.ok) {
      throw new ApiProblem(await decodeProblem(response));
    }
    if (response.status === 204 || response.status === 205) {
      return undefined as T;
    }
    return (await response.json()) as T;
  },
};
