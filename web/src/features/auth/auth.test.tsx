import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { expect, test } from "vitest";

import { renderApp } from "../../test/render";
import { server } from "../../test/server";
import { register } from "./api";

const apiOrigin = "*";
const sessionId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e";
const userId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a26d";

function unauthenticatedProblem() {
  return HttpResponse.json(
    {
      type: "https://folioharbor.example/problems/unauthenticated",
      title: "Authentication required",
      status: 401,
      detail: "Authentication is required.",
      instance: "/problems/01K1AUTHFLOW00000000000000",
      code: "unauthenticated",
      request_id: "01K1AUTHFLOW00000000000000",
    },
    { status: 401, headers: { "Content-Type": "application/problem+json" } },
  );
}

function problem(code: string, requestId: string, status = 422) {
  return {
    type: `https://folioharbor.example/problems/${code.replaceAll("_", "-")}`,
    title: "Request failed",
    status,
    detail: "This server detail must not be rendered by the client.",
    instance: `/problems/${requestId}`,
    code,
    request_id: requestId,
  };
}

function matchesBody(value: unknown, expected: Readonly<Record<string, string>>): boolean {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return Object.keys(record).length === Object.keys(expected).length &&
    Object.entries(expected).every(([key, expectedValue]) => record[key] === expectedValue);
}

test("the HTTP boundary sends a generated registration DTO and decodes the accepted response", async () => {
  server.use(
    http.post(`${apiOrigin}/api/v1/auth/register`, async ({ request }) => {
      const body: unknown = await request.json();
      return matchesBody(body, { email: "reader@example.com", password: "safe-password-123" })
        ? HttpResponse.json({ status: "pending" }, { status: 202 })
        : HttpResponse.json(problem("invalid_json_body", "01K1BODY000000000000000000"), { status: 422 });
    }),
  );

  await expect(register({ email: "reader@example.com", password: "safe-password-123" })).resolves.toMatchObject({
    status: "pending",
  });
});

test("a reader can register, verify their email, log in, and reach the authenticated shell", async () => {
  let authenticated = false;

  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () => {
      if (!authenticated) {
        return unauthenticatedProblem();
      }
      return HttpResponse.json({ session_id: sessionId, is_current: true, status: "active" });
    }),
    http.post(`${apiOrigin}/api/v1/auth/register`, async ({ request }) => {
      const body: unknown = await request.json();
      if (!matchesBody(body, { email: "reader@example.com", password: "safe-password-123" })) {
        return HttpResponse.json({ code: "invalid_json_body" }, { status: 422 });
      }
      return HttpResponse.json({ status: "pending" }, { status: 202 });
    }),
    http.post(`${apiOrigin}/api/v1/auth/verify-email`, async ({ request }) => {
      const body: unknown = await request.json();
      if (!matchesBody(body, { token: "verification-token" })) {
        return HttpResponse.json({ code: "invalid_json_body" }, { status: 422 });
      }
      return new HttpResponse(null, { status: 204 });
    }),
    http.post(`${apiOrigin}/api/v1/auth/login`, async ({ request }) => {
      const body: unknown = await request.json();
      if (!matchesBody(body, { email: "reader@example.com", password: "safe-password-123" })) {
        return HttpResponse.json({ code: "invalid_json_body" }, { status: 422 });
      }
      authenticated = true;
      return HttpResponse.json({ user_id: userId, session_id: sessionId });
    }),
  );

  const user = userEvent.setup();
  renderApp("/register");

  await user.type(await screen.findByLabelText("Email"), "reader@example.com");
  await user.type(screen.getByLabelText("Password"), "safe-password-123");
  await user.click(screen.getByRole("button", { name: "Create account" }));

  expect(await screen.findByText(/Verification required/)).toBeInTheDocument();

  await user.click(screen.getByRole("link", { name: "Verify email" }));
  await user.type(screen.getByLabelText("Verification token"), "verification-token");
  await user.click(screen.getByRole("button", { name: "Verify email" }));

  expect(await screen.findByText(/Email verified/)).toBeInTheDocument();

  await user.click(screen.getByRole("link", { name: "Log in" }));
  await user.type(screen.getByLabelText("Email"), "reader@example.com");
  await user.type(screen.getByLabelText("Password"), "safe-password-123");
  await user.click(screen.getByRole("button", { name: "Log in" }));

  expect(await screen.findByRole("heading", { name: "FolioHarbor" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Sessions" })).toBeInTheDocument();
});

test("auth errors use localized stable codes and unknown errors expose only a safe message and request ID", async () => {
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, unauthenticatedProblem),
    http.post(`${apiOrigin}/api/v1/auth/login`, () =>
      HttpResponse.json(problem("email_verification_required", "01K1VERIFY0000000000000000", 409), {
        status: 409,
        headers: { "Content-Type": "application/problem+json" },
      }),
    ),
  );

  const user = userEvent.setup();
  renderApp("/login");

  await user.selectOptions(screen.getByLabelText("Language"), "zh-CN");
  expect(screen.getByRole("heading", { name: "登录" })).toBeInTheDocument();

  await user.type(screen.getByLabelText("邮箱"), "reader@example.com");
  await user.type(screen.getByLabelText("密码"), "safe-password-123");
  await user.click(screen.getByRole("button", { name: "登录" }));

  const knownError = await screen.findByRole("alert");
  expect(knownError).toHaveTextContent("请先验证邮箱");
  expect(knownError).not.toHaveTextContent("server detail");

  server.use(
    http.post(`${apiOrigin}/api/v1/auth/login`, () =>
      HttpResponse.json(problem("future_problem", "01K1UNKNOWN000000000000000", 500), {
        status: 500,
        headers: { "Content-Type": "application/problem+json" },
      }),
    ),
  );

  await user.click(screen.getByRole("button", { name: "登录" }));
  await screen.findByText(/01K1UNKNOWN000000000000000/);
  const unknownError = screen.getByRole("alert");
  expect(unknownError).toHaveTextContent("请求无法完成");
  expect(unknownError).toHaveTextContent("01K1UNKNOWN000000000000000");
  expect(unknownError).not.toHaveTextContent("server detail");
});

test("client validation links field errors and focuses the error summary", async () => {
  server.use(http.get(`${apiOrigin}/api/v1/auth/session`, unauthenticatedProblem));

  const user = userEvent.setup();
  renderApp("/register");

  await user.click(await screen.findByRole("button", { name: "Create account" }));

  const summary = await screen.findByRole("alert");
  expect(summary).toHaveFocus();
  expect(screen.getByLabelText("Email")).toHaveAccessibleDescription("Email is required.");
  expect(screen.getByLabelText("Password")).toHaveAccessibleDescription("Password is required.");
});

test("repeated client validation refocuses an unchanged error summary", async () => {
  server.use(http.get(`${apiOrigin}/api/v1/auth/session`, unauthenticatedProblem));

  const user = userEvent.setup();
  renderApp("/register");

  const submit = await screen.findByRole("button", { name: "Create account" });
  await user.click(submit);
  const summary = await screen.findByRole("alert");
  expect(summary).toHaveFocus();

  await user.click(submit);
  expect(summary).toHaveFocus();
});

test("password recovery stays non-enumerating and reset establishes a new authenticated session", async () => {
  let authenticated = false;

  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      authenticated
        ? HttpResponse.json({ session_id: sessionId, is_current: true, status: "active" })
        : unauthenticatedProblem(),
    ),
    http.post(`${apiOrigin}/api/v1/auth/forgot-password`, () =>
      HttpResponse.json({ status: "accepted" }, { status: 202 }),
    ),
    http.post(`${apiOrigin}/api/v1/auth/reset-password`, async ({ request }) => {
      const body: unknown = await request.json();
      if (!matchesBody(body, { token: "reset-token", new_password: "new-safe-password-123" })) {
        return HttpResponse.json(problem("invalid_json_body", "01K1BODY000000000000000000"), { status: 422 });
      }
      authenticated = true;
      return HttpResponse.json({ user_id: userId, session_id: sessionId });
    }),
  );

  const user = userEvent.setup();
  renderApp("/forgot-password");

  await user.type(await screen.findByLabelText("Email"), "unknown@example.com");
  await user.click(screen.getByRole("button", { name: "Send reset instructions" }));
  expect(await screen.findByText(/If an account exists/)).toBeInTheDocument();

  await user.click(screen.getByRole("link", { name: "Reset password" }));
  await user.type(screen.getByLabelText("Reset token"), "reset-token");
  await user.type(screen.getByLabelText("New password"), "new-safe-password-123");
  await user.click(screen.getByRole("button", { name: "Reset password" }));

  expect(await screen.findByRole("heading", { name: "FolioHarbor" })).toBeInTheDocument();
});

test("account B never sees account A's cached session list", async () => {
  const accountASessionId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a272";
  const accountBSessionId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a273";
  const accountBUserId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a274";
  let authenticatedAccount: "a" | "b" | null = "a";
  let releaseAccountBSessions: (() => void) | undefined;
  const accountBSessionsPending = new Promise<void>((resolve) => {
    releaseAccountBSessions = resolve;
  });

  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () => {
      if (authenticatedAccount === null) {
        return unauthenticatedProblem();
      }
      const currentSessionId = authenticatedAccount === "a" ? accountASessionId : accountBSessionId;
      return HttpResponse.json({ session_id: currentSessionId, is_current: true, status: "active" });
    }),
    http.get(`${apiOrigin}/api/v1/auth/sessions`, async () => {
      const account = authenticatedAccount;
      if (account === null) {
        return unauthenticatedProblem();
      }
      if (account === "b") {
        await accountBSessionsPending;
      }
      const currentSessionId = account === "a" ? accountASessionId : accountBSessionId;
      return HttpResponse.json([{ session_id: currentSessionId, is_current: true, status: "active" }]);
    }),
    http.post(`${apiOrigin}/api/v1/auth/logout`, () => {
      authenticatedAccount = null;
      return new HttpResponse(null, { status: 204 });
    }),
    http.post(`${apiOrigin}/api/v1/auth/login`, async ({ request }) => {
      const body: unknown = await request.json();
      if (!matchesBody(body, { email: "account-b@example.com", password: "safe-password-456" })) {
        return HttpResponse.json(problem("invalid_json_body", "01K1BODY000000000000000001"), { status: 422 });
      }
      authenticatedAccount = "b";
      return HttpResponse.json({ user_id: accountBUserId, session_id: accountBSessionId });
    }),
  );

  const user = userEvent.setup();
  renderApp("/account/sessions");

  expect(await screen.findByText(accountASessionId)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Log out" }));
  await screen.findByRole("heading", { name: "Log in" });

  await user.type(screen.getByLabelText("Email"), "account-b@example.com");
  await user.type(screen.getByLabelText("Password"), "safe-password-456");
  await user.click(screen.getByRole("button", { name: "Log in" }));
  await screen.findByRole("link", { name: "Sessions" });
  await user.click(screen.getByRole("link", { name: "Sessions" }));
  await screen.findByRole("heading", { name: "Account sessions" });

  try {
    expect(screen.queryByText(accountASessionId)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: `Revoke ${accountASessionId}` })).not.toBeInTheDocument();
  } finally {
    releaseAccountBSessions?.();
  }
  expect(await screen.findByText(accountBSessionId)).toBeInTheDocument();
});

test("the account page lists sessions and sends CSRF-protected revoke-one and revoke-all requests", async () => {
  const otherSessionId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a270";
  const activeSessions = new Set([sessionId, otherSessionId]);

  document.cookie = "folioharbor_csrf=csrf-token; Path=/";
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      activeSessions.has(sessionId)
        ? HttpResponse.json({ session_id: sessionId, is_current: true, status: "active" })
        : unauthenticatedProblem(),
    ),
    http.get(`${apiOrigin}/api/v1/auth/sessions`, () =>
      HttpResponse.json(
        [...activeSessions].map((id) => ({ session_id: id, is_current: id === sessionId, status: "active" })),
      ),
    ),
    http.post(`${apiOrigin}/api/v1/auth/sessions/:sessionId/revoke`, ({ params, request }) => {
      if (request.headers.get("X-CSRF-Token") !== "csrf-token") {
        return HttpResponse.json(problem("csrf_failed", "01K1CSRF00000000000000000", 403), { status: 403 });
      }
      const id = String(params.sessionId);
      activeSessions.delete(id);
      return new HttpResponse(null, { status: 204 });
    }),
  );

  const user = userEvent.setup();
  renderApp("/account/sessions");

  expect(await screen.findByText(otherSessionId)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: `Revoke ${otherSessionId}` }));
  expect(await screen.findByText("Session revoked.")).toBeInTheDocument();
  expect(screen.queryByText(otherSessionId)).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Revoke all sessions" }));
  expect(await screen.findByRole("heading", { name: "Log in" })).toBeInTheDocument();
});

test("revoking the current session through its row returns to the anonymous login route", async () => {
  let currentSessionActive = true;

  document.cookie = "folioharbor_csrf=csrf-token; Path=/";
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      currentSessionActive
        ? HttpResponse.json({ session_id: sessionId, is_current: true, status: "active" })
        : unauthenticatedProblem(),
    ),
    http.get(`${apiOrigin}/api/v1/auth/sessions`, () =>
      HttpResponse.json(
        currentSessionActive
          ? [{ session_id: sessionId, is_current: true, status: "active" }]
          : [],
      ),
    ),
    http.post(`${apiOrigin}/api/v1/auth/sessions/:sessionId/revoke`, ({ params }) => {
      if (String(params.sessionId) === sessionId) {
        currentSessionActive = false;
      }
      return new HttpResponse(null, { status: 204 });
    }),
  );

  const user = userEvent.setup();
  renderApp("/account/sessions");

  await user.click(await screen.findByRole("button", { name: `Revoke ${sessionId}` }));

  expect(await screen.findByRole("heading", { name: "Log in" })).toBeInTheDocument();
});

test("a revoked session history row has no revoke-one action", async () => {
  const revokedSessionId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a271";

  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json({ session_id: sessionId, is_current: true, status: "active" }),
    ),
    http.get(`${apiOrigin}/api/v1/auth/sessions`, () =>
      HttpResponse.json([
        { session_id: sessionId, is_current: true, status: "active" },
        { session_id: revokedSessionId, is_current: false, status: "revoked" },
      ]),
    ),
  );

  renderApp("/account/sessions");

  expect(await screen.findByText(revokedSessionId)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: `Revoke ${revokedSessionId}` })).not.toBeInTheDocument();
});

test("revoked session history does not prevent revoke-all from revoking the current session", async () => {
  const revokedSessionId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a271";
  let currentSessionActive = true;

  document.cookie = "folioharbor_csrf=csrf-token; Path=/";
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      currentSessionActive
        ? HttpResponse.json({ session_id: sessionId, is_current: true, status: "active" })
        : unauthenticatedProblem(),
    ),
    http.get(`${apiOrigin}/api/v1/auth/sessions`, () =>
      HttpResponse.json([
        { session_id: revokedSessionId, is_current: false, status: "revoked" },
        { session_id: sessionId, is_current: true, status: "active" },
      ]),
    ),
    http.post(`${apiOrigin}/api/v1/auth/sessions/:sessionId/revoke`, ({ params }) => {
      const id = String(params.sessionId);
      if (id === revokedSessionId) {
        return HttpResponse.json(problem("session_not_found", "01K1SESSIONHISTORY000000000", 404), {
          status: 404,
          headers: { "Content-Type": "application/problem+json" },
        });
      }
      if (id === sessionId) {
        currentSessionActive = false;
      }
      return new HttpResponse(null, { status: 204 });
    }),
  );

  const user = userEvent.setup();
  renderApp("/account/sessions");

  await user.click(await screen.findByRole("button", { name: "Revoke all sessions" }));

  expect(await screen.findByRole("heading", { name: "Log in" })).toBeInTheDocument();
});

test.each([
  { blockedHeading: "Log in", route: "/login" },
  { blockedHeading: "Account sessions", route: "/account/sessions" },
])("a session-load failure stays on $route and renders a safe error", async ({ blockedHeading, route }) => {
  const requestId = "01K1SESSIONLOAD0000000000000";
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json(problem("identity_service_unavailable", requestId, 500), {
        status: 500,
        headers: { "Content-Type": "application/problem+json" },
      }),
    ),
  );

  renderApp(route);

  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("The request could not be completed.");
  expect(alert).toHaveTextContent(requestId);
  expect(screen.queryByRole("heading", { name: blockedHeading })).not.toBeInTheDocument();
  expect(window.location.pathname).toBe(route);
});
