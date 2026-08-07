import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { HttpResponse, http } from "msw";
import { expect, test } from "vitest";

import { renderApp } from "../../test/render";
import { server } from "../../test/server";

const apiOrigin = "*";
const personalId = "018f47b5-58b4-7ba6-9a3a-d9f41f17b001";
const sharedId = "018f47b5-58b4-7ba6-9a3a-d9f41f17b002";
const firstItemId = "018f47b5-58b4-7ba6-9a3a-d9f41f17c001";
const secondItemId = "018f47b5-58b4-7ba6-9a3a-d9f41f17c002";
const accountAUploadId = "018f47b5-58b4-7ba6-9a3a-d9f41f17e001";
const accountAMemberId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a275";

const owner = {
  library_id: personalId,
  name: "Personal Library",
  role: "owner",
  reader_download_enabled: false,
  capabilities: {
    can_upload: true,
    can_invite_members: true,
    can_manage_members: true,
    can_manage_settings: true,
  },
} as const;

const reader = {
  library_id: sharedId,
  name: "Shared Library",
  role: "reader",
  reader_download_enabled: false,
  capabilities: {
    can_upload: false,
    can_invite_members: false,
    can_manage_members: false,
    can_manage_settings: false,
  },
} as const;

function authenticatedLibraryHandlers(libraries: readonly [typeof owner, typeof reader] = [owner, reader]) {
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json({
        session_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e",
        is_current: true,
        status: "active",
      }),
    ),
    http.get(`${apiOrigin}/api/v1/libraries`, () => HttpResponse.json(libraries)),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId`, ({ params }) => {
      const library = libraries.find(({ library_id }) => library_id === String(params.libraryId));
      return library === undefined ? new HttpResponse(null, { status: 404 }) : HttpResponse.json(library);
    }),
  );
}

function book(item_id: string, primary_title: string, can_download: boolean) {
  return {
    item_id,
    primary_title,
    authors: ["Ursula Reader"],
    languages: ["en"],
    media_type: "application/epub+zip",
    can_read: true,
    can_download,
  };
}

test("two-library switching keeps the route-selected library visible and has no global all-books link", async () => {
  authenticatedLibraryHandlers();
  server.use(
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/books`, ({ params }) =>
      HttpResponse.json({
        items: [
          String(params.libraryId) === personalId
            ? book(firstItemId, "Personal Book", true)
            : book(secondItemId, "Shared Book", false),
        ],
      }),
    ),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${personalId}/books`);

  const switcher = await screen.findByRole("combobox", { name: "Current library" });
  expect(switcher).toHaveValue(personalId);
  expect(screen.getByRole("option", { name: "Personal Library" })).toBeInTheDocument();
  expect(screen.getByRole("option", { name: "Shared Library" })).toBeInTheDocument();
  expect(await screen.findByRole("heading", { name: "All books" })).toBeInTheDocument();
  expect(screen.getByText("Personal Book")).toBeInTheDocument();

  await user.selectOptions(switcher, sharedId);
  expect(window.location.pathname).toBe(`/libraries/${sharedId}/books`);
  expect(await screen.findByText("Shared Book")).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Shared Library" })).toBeInTheDocument();

  for (const link of screen.getAllByRole("link")) {
    expect(link.getAttribute("href")).not.toBe("/books");
  }
});

test("a direct item link survives reload and presents user concepts and independent capabilities", async () => {
  authenticatedLibraryHandlers();
  server.use(
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/items/:itemId`, () =>
      HttpResponse.json({
        ...book(secondItemId, "Shared Book", false),
        manifestation_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17d001",
        identifiers: ["urn:isbn:9780000000000"],
      }),
    ),
  );

  renderApp(`/libraries/${sharedId}/items/${secondItemId}`);

  expect(await screen.findByRole("heading", { name: "Shared Book" })).toBeInTheDocument();
  expect(screen.getByText("Work")).toBeInTheDocument();
  expect(screen.getByText("Edition / format")).toBeInTheDocument();
  expect(screen.getByText("Library copy")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Read online" })).toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Download EPUB" })).not.toBeInTheDocument();
  expect(screen.getByText("Online reading only")).toBeInTheDocument();
  expect(document.body).not.toHaveTextContent(/\b(?:Blob|Package|Work ID|storage)\b/i);
  expect((await axe.run(document.body)).violations).toEqual([]);
});

test("catalog pagination loads the opaque next page through an accessible control", async () => {
  authenticatedLibraryHandlers();
  const requestedCursors: (string | null)[] = [];
  server.use(
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/books`, ({ request }) => {
      const cursor = new URL(request.url).searchParams.get("cursor");
      requestedCursors.push(cursor);
      return cursor === null
        ? HttpResponse.json({ items: [book(firstItemId, "First Book", true)], next_cursor: "opaque-next" })
        : HttpResponse.json({ items: [book(secondItemId, "Second Book", true)] });
    }),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${personalId}/books`);

  expect(await screen.findByText("First Book")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Load more books" }));
  expect(await screen.findByText("Second Book")).toBeInTheDocument();
  expect(requestedCursors).toEqual([null, "opaque-next"]);
  expect(within(screen.getByRole("list", { name: "Books" })).getAllByRole("listitem")).toHaveLength(2);
});

test("server capabilities control owner, editor, and reader navigation without becoming authorization", async () => {
  const editor = {
    ...reader,
    library_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17b003",
    name: "Edited Library",
    role: "editor",
    capabilities: { ...reader.capabilities, can_upload: true },
  } as const;
  const libraries = [owner, editor, reader] as const;
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json({ session_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e", is_current: true, status: "active" }),
    ),
    http.get(`${apiOrigin}/api/v1/libraries`, () => HttpResponse.json(libraries)),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId`, ({ params }) =>
      HttpResponse.json(libraries.find(({ library_id }) => library_id === String(params.libraryId))),
    ),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/books`, () => HttpResponse.json({ items: [] })),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${personalId}/books`);
  await screen.findByRole("heading", { name: "All books" });
  expect(screen.getByRole("link", { name: "Uploads" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Members" })).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Library settings" })).toBeInTheDocument();

  await user.selectOptions(screen.getByRole("combobox", { name: "Current library" }), editor.library_id);
  await screen.findByRole("heading", { name: "Edited Library" });
  expect(screen.getByRole("link", { name: "Uploads" })).toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Members" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Library settings" })).not.toBeInTheDocument();

  await user.selectOptions(screen.getByRole("combobox", { name: "Current library" }), sharedId);
  await screen.findByRole("heading", { name: "Shared Library" });
  expect(screen.queryByRole("link", { name: "Uploads" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Members" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Library settings" })).not.toBeInTheDocument();
  expect(screen.getByText("Your role: Reader")).toBeInTheDocument();
});

test("account B never sees account A library resources while B responses are pending", async () => {
  const accountBId = "018f47b5-58b4-7ba6-9a3a-d9f41f17b004";
  const accountBSessionId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a276";
  const accountBUserId = "018f47b5-58b4-7ba6-9a3a-d9f41f17a277";
  const accountBLibrary = {
    ...reader,
    library_id: accountBId,
    name: "Account B Library",
  } as const;
  let authenticatedAccount: "a" | "b" | null = "a";
  let releaseAccountBResponses: (() => void) | undefined;
  const accountBResponsesPending = new Promise<void>((resolve) => {
    releaseAccountBResponses = resolve;
  });

  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () => {
      if (authenticatedAccount === null) {
        return HttpResponse.json(
          { status: 401, code: "unauthenticated", request_id: "account-switch" },
          { status: 401, headers: { "Content-Type": "application/problem+json" } },
        );
      }
      return HttpResponse.json({
        session_id: authenticatedAccount === "a"
          ? "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e"
          : accountBSessionId,
        is_current: true,
        status: "active",
      });
    }),
    http.get(`${apiOrigin}/api/v1/libraries`, async () => {
      if (authenticatedAccount === "b") {
        await accountBResponsesPending;
        return HttpResponse.json([accountBLibrary]);
      }
      return HttpResponse.json([owner]);
    }),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId`, async ({ params }) => {
      if (authenticatedAccount === "b") {
        await accountBResponsesPending;
        return String(params.libraryId) === accountBId
          ? HttpResponse.json(accountBLibrary)
          : new HttpResponse(null, { status: 404 });
      }
      return HttpResponse.json(owner);
    }),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/books`, async () => {
      if (authenticatedAccount === "b") {
        await accountBResponsesPending;
        return HttpResponse.json({ items: [] });
      }
      return HttpResponse.json({ items: [book(firstItemId, "Account A Book", true)] });
    }),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/members`, async () => {
      if (authenticatedAccount === "b") {
        await accountBResponsesPending;
        return HttpResponse.json([]);
      }
      return HttpResponse.json([{ user_id: accountAMemberId, role: "reader" }]);
    }),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/uploads/:uploadId`, async () => {
      if (authenticatedAccount === "b") {
        await accountBResponsesPending;
      }
      return HttpResponse.json({
        upload_id: accountAUploadId,
        library_id: personalId,
        file_name: authenticatedAccount === "a" ? "account-a.epub" : "account-b.epub",
        media_type: "application/epub+zip",
        declared_bytes: 10,
        received_bytes: 10,
        state: "ready",
        status_url: `/api/v1/libraries/${personalId}/uploads/${accountAUploadId}`,
        error_code: null,
        item_id: firstItemId,
      });
    }),
    http.post(`${apiOrigin}/api/v1/auth/logout`, () => {
      authenticatedAccount = null;
      return new HttpResponse(null, { status: 204 });
    }),
    http.post(`${apiOrigin}/api/v1/auth/login`, () => {
      authenticatedAccount = "b";
      return HttpResponse.json({ user_id: accountBUserId, session_id: accountBSessionId });
    }),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${personalId}/books`);
  expect(await screen.findByText("Account A Book")).toBeInTheDocument();

  navigateInBrowser(`/libraries/${personalId}/members`);
  expect(await screen.findByText(accountAMemberId)).toBeInTheDocument();

  navigateInBrowser(`/libraries/${personalId}/uploads?upload=${accountAUploadId}`);
  expect(await screen.findByRole("heading", { name: "account-a.epub" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Log out" }));
  await user.type(await screen.findByLabelText("Email"), "account-b@example.com");
  await user.type(screen.getByLabelText("Password"), "safe-password-456");
  await user.click(screen.getByRole("button", { name: "Log in" }));
  await screen.findByRole("link", { name: "Sessions" });

  try {
    assertAccountAResourcesAreHidden();

    navigateInBrowser(`/libraries/${personalId}/members`);
    await waitFor(() => { expect(window.location.pathname).toBe(`/libraries/${personalId}/members`); });
    assertAccountAResourcesAreHidden();

    navigateInBrowser(`/libraries/${personalId}/uploads?upload=${accountAUploadId}`);
    await waitFor(() => { expect(window.location.pathname).toBe(`/libraries/${personalId}/uploads`); });
    assertAccountAResourcesAreHidden();
  } finally {
    releaseAccountBResponses?.();
  }
});

test("a route library absent from the visible list is never promoted into authenticated state", async () => {
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json({
        session_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e",
        is_current: true,
        status: "active",
      }),
    ),
    http.get(`${apiOrigin}/api/v1/libraries`, () => HttpResponse.json([reader])),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId`, ({ params }) =>
      HttpResponse.json(String(params.libraryId) === personalId ? owner : reader),
    ),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/books`, () => HttpResponse.json({ items: [] })),
  );

  renderApp(`/libraries/${personalId}/books`);

  await waitFor(() => { expect(window.location.pathname).toBe(`/libraries/${sharedId}/books`); });
  expect(await screen.findByRole("heading", { name: "Shared Library" })).toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "Personal Library" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Uploads" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Members" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Library settings" })).not.toBeInTheDocument();
});

function navigateInBrowser(route: string) {
  act(() => {
    window.history.pushState({}, "", route);
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
}

function assertAccountAResourcesAreHidden() {
  expect(screen.queryAllByText("Personal Library")).toHaveLength(0);
  expect(screen.queryByText("Account A Book")).not.toBeInTheDocument();
  expect(screen.queryByText(accountAMemberId)).not.toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "account-a.epub" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Uploads" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Members" })).not.toBeInTheDocument();
  expect(screen.queryByRole("link", { name: "Library settings" })).not.toBeInTheDocument();
}
