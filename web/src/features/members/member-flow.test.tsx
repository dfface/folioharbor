import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { HttpResponse, http } from "msw";
import { expect, test } from "vitest";

import { renderApp } from "../../test/render";
import { server } from "../../test/server";

const apiOrigin = "*";
const libraryId = "018f47b5-58b4-7ba6-9a3a-d9f41f17b201";
const personalId = "018f47b5-58b4-7ba6-9a3a-d9f41f17b202";
const ownerId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a201";
const readerId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a202";

const ownerLibrary = {
  library_id: libraryId,
  name: "Team Library",
  role: "owner",
  reader_download_enabled: false,
  capabilities: {
    can_upload: true,
    can_invite_members: true,
    can_manage_members: true,
    can_manage_settings: true,
  },
} as const;

const personalLibrary = { ...ownerLibrary, library_id: personalId, name: "Personal Library" } as const;

function authenticatedHandlers(libraries: readonly unknown[] = [ownerLibrary]) {
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json({ session_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e", is_current: true, status: "active" }),
    ),
    http.get(`${apiOrigin}/api/v1/libraries`, () => HttpResponse.json(libraries)),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId`, ({ params }) =>
      HttpResponse.json(String(params.libraryId) === personalId ? personalLibrary : ownerLibrary),
    ),
  );
}

function problem(code: string, status: number) {
  return {
    type: `https://folioharbor.example/problems/${code.replaceAll("_", "-")}`,
    title: "Request failed",
    status,
    detail: "Server detail must not be displayed.",
    instance: "/problems/01K1MEMBERS00000000000000",
    code,
    request_id: "01K1MEMBERS00000000000000",
  };
}

test("an owner invites, changes roles, removes members, and relies on the server for final-owner protection", async () => {
  authenticatedHandlers();
  let invitedBody: unknown;
  let roleBody: unknown;
  let readerRemoved = false;
  server.use(
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/members`, () =>
      HttpResponse.json([
        { user_id: ownerId, role: "owner" },
        ...(readerRemoved ? [] : [{ user_id: readerId, role: "reader" }]),
      ]),
    ),
    http.post(`${apiOrigin}/api/v1/libraries/:libraryId/invitations`, async ({ request }) => {
      invitedBody = await request.json();
      return new HttpResponse(null, { status: 204 });
    }),
    http.patch(`${apiOrigin}/api/v1/libraries/:libraryId/members/:userId`, async ({ request }) => {
      roleBody = await request.json();
      return new HttpResponse(null, { status: 204 });
    }),
    http.delete(`${apiOrigin}/api/v1/libraries/:libraryId/members/:userId`, ({ params }) => {
      if (String(params.userId) === ownerId) {
        return HttpResponse.json(problem("library_requires_owner", 409), {
          status: 409,
          headers: { "Content-Type": "application/problem+json" },
        });
      }
      readerRemoved = true;
      return new HttpResponse(null, { status: 204 });
    }),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${libraryId}/members`);

  await user.type(await screen.findByLabelText("Invitee email"), "new-reader@example.com");
  await user.selectOptions(screen.getByLabelText("Invitation role"), "editor");
  await user.click(screen.getByRole("button", { name: "Send invitation" }));
  expect(await screen.findByText("Invitation sent.")).toBeInTheDocument();
  expect(invitedBody).toEqual({ email: "new-reader@example.com", role: "editor" });

  await user.selectOptions(screen.getByLabelText(`Role for ${readerId}`), "editor");
  expect(await screen.findByText("Member role updated.")).toBeInTheDocument();
  expect(roleBody).toEqual({ role: "editor" });

  await user.click(screen.getByRole("button", { name: `Remove ${ownerId}` }));
  expect(await screen.findByRole("alert")).toHaveTextContent("A library must keep at least one owner.");
  expect(screen.getByText(ownerId)).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: `Remove ${readerId}` }));
  expect(await screen.findByText("Member removed.")).toBeInTheDocument();
  expect(screen.queryByText(readerId)).not.toBeInTheDocument();
  expect((await axe.run(document.body)).violations).toEqual([]);
});

test("library settings update the reader download toggle without merging reading permission", async () => {
  authenticatedHandlers();
  let settingsBody: unknown;
  server.use(
    http.patch(`${apiOrigin}/api/v1/libraries/:libraryId/settings`, async ({ request }) => {
      settingsBody = await request.json();
      return new HttpResponse(null, { status: 204 });
    }),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${libraryId}/settings`);
  const toggle = await screen.findByRole("checkbox", { name: "Allow readers to download original EPUB files" });
  expect(toggle).not.toBeChecked();
  await user.click(toggle);
  await user.click(screen.getByRole("button", { name: "Save library settings" }));

  expect(await screen.findByText("Library settings saved.")).toBeInTheDocument();
  expect(settingsBody).toEqual({ name: "Team Library", reader_download_enabled: true });
  expect(screen.getByText("Online reading permission remains separate.")).toBeInTheDocument();
});

test("a logged-out invitation keeps its token in a safe return link", async () => {
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json(problem("unauthenticated", 401), {
        status: 401,
        headers: { "Content-Type": "application/problem+json" },
      }),
    ),
  );

  renderApp("/invitations/invitation-token");

  const link = await screen.findByRole("link", { name: "Log in to accept invitation" });
  expect(link).toHaveAttribute("href", "/login?returnTo=%2Finvitations%2Finvitation-token");
  expect((await axe.run(document.body)).violations).toEqual([]);
});

test("successful invitation acceptance keeps personal and shared libraries in the switcher", async () => {
  let accepted = false;
  authenticatedHandlers([personalLibrary]);
  server.use(
    http.get(`${apiOrigin}/api/v1/libraries`, () =>
      HttpResponse.json(accepted ? [personalLibrary, ownerLibrary] : [personalLibrary]),
    ),
    http.post(`${apiOrigin}/api/v1/invitations/accept`, () => {
      accepted = true;
      return HttpResponse.json({ status: "accepted", library_id: libraryId });
    }),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/books`, () => HttpResponse.json({ items: [] })),
  );

  const user = userEvent.setup();
  renderApp("/invitations/invitation-token");
  await user.click(await screen.findByRole("button", { name: "Accept invitation" }));

  expect(await screen.findByRole("heading", { name: "Team Library" })).toBeInTheDocument();
  expect(screen.getByRole("option", { name: "Personal Library" })).toBeInTheDocument();
  expect(screen.getByRole("option", { name: "Team Library" })).toBeInTheDocument();
  expect(window.location.pathname).toBe(`/libraries/${libraryId}/books`);
});

test.each([
  [{ status: "wrong_account", email_hint: "r***@example.com" }, "This invitation is for r***@example.com. Switch accounts to continue."],
  [{ status: "unverified" }, "Verify your email before accepting this invitation."],
  [{ status: "expired" }, "This invitation has expired."],
  [{ status: "consumed" }, "This invitation has already been used."],
  [{ status: "invalid" }, "This invitation is not valid."],
] as const)("invitation state $status is explained without server detail", async (response, copy) => {
  authenticatedHandlers();
  server.use(http.post(`${apiOrigin}/api/v1/invitations/accept`, () => HttpResponse.json(response)));

  const user = userEvent.setup();
  renderApp("/invitations/invitation-token");
  await user.click(await screen.findByRole("button", { name: "Accept invitation" }));

  expect(await screen.findByRole("status")).toHaveTextContent(copy);
  expect(screen.queryByText(/Server detail/)).not.toBeInTheDocument();
});
