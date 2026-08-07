import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import i18n from "../../i18n";
import { renderApp } from "../../test/render";
import { server } from "../../test/server";
import { createLocator } from "./locator";

const apiOrigin = "*";
const libraryId = "018f47b5-58b4-7ba6-9a3a-d9f41f17b001";
const itemId = "018f47b5-58b4-7ba6-9a3a-d9f41f17c001";
const manifestationId = "018f47b5-58b4-7ba6-9a3a-d9f41f17d001";
const firstHref = `/api/v1/items/${itemId}/resources/chapter-one`;
const secondHref = `/api/v1/items/${itemId}/resources/chapter-two`;

const library = {
  library_id: libraryId,
  name: "Personal Library",
  role: "owner",
  is_personal: true,
  capabilities: {
    can_view_catalog: true,
    can_upload: true,
    can_invite_members: true,
    can_manage_members: true,
    can_manage_settings: true,
  },
};

const manifest = {
  metadata: { title: "Safe Book", authors: ["Writer"], languages: ["en"] },
  manifestationId,
  readingOrder: [
    { href: firstHref, type: "application/xhtml+xml" },
    { href: secondHref, type: "application/xhtml+xml" },
  ],
  resources: [{ href: `/api/v1/items/${itemId}/resources/styles`, type: "text/css", rel: "resource" }],
  toc: [
    { href: `${firstHref}#start`, type: "application/xhtml+xml", title: "Chapter one" },
    { href: `${secondHref}#middle`, type: "application/xhtml+xml", title: "Chapter two" },
  ],
  links: [{ href: `/api/v1/items/${itemId}/manifest`, type: "application/webpub+json", rel: "self" }],
};

let createdObjectUrls: Blob[];
let revokedObjectUrls: string[];

function problem(status: number, code: string) {
  return HttpResponse.json(
    {
      type: `https://folioharbor.test/problems/${code}`,
      title: "Request failed",
      status,
      detail: "The request could not be completed.",
      instance: "/problems/test-request",
      code,
      request_id: "test-request",
    },
    { status, headers: { "Content-Type": "application/problem+json" } },
  );
}

function installReaderHandlers(overrides: { manifest?: typeof manifest; resourceStatus?: number } = {}) {
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json({ session_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e", is_current: true, status: "active" }),
    ),
    http.get(`${apiOrigin}/api/v1/libraries`, () => HttpResponse.json([library])),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId`, () => HttpResponse.json(library)),
    http.get(`${apiOrigin}/api/v1/items/:itemId/manifest`, () => HttpResponse.json(overrides.manifest ?? manifest)),
    http.get(`${apiOrigin}/api/v1/items/:itemId/resources/:resourceId`, ({ params }) => {
      if (overrides.resourceStatus !== undefined) {
        return problem(overrides.resourceStatus, "item_not_found");
      }
      return new HttpResponse(
        `<html><body><h1>EPUB ${String(params.resourceId)}</h1></body></html>`,
        { headers: { "Content-Type": "application/xhtml+xml" } },
      );
    }),
    http.get(`${apiOrigin}/api/v1/manifestations/:manifestationId/progress`, () =>
      new HttpResponse(null, { status: 204, headers: { ETag: '"progress-v0"' } }),
    ),
    http.put(`${apiOrigin}/api/v1/manifestations/:manifestationId/progress`, async ({ request }) => {
      const body = await request.json() as { baseVersion: number; locator: unknown };
      return HttpResponse.json({
        manifestationId,
        locator: body.locator,
        version: body.baseVersion + 1,
        updatedAt: "2026-08-07T00:00:00Z",
      });
    }),
  );
}

beforeEach(() => {
  createdObjectUrls = [];
  revokedObjectUrls = [];
  Object.defineProperties(URL, {
    createObjectURL: { configurable: true, value: vi.fn((blob: Blob) => {
      createdObjectUrls.push(blob);
      return `blob:http://localhost/reader-${String(createdObjectUrls.length)}`;
    }) },
    revokeObjectURL: { configurable: true, value: vi.fn((url: string) => {
      revokedObjectUrls.push(url);
    }) },
  });
  vi.stubGlobal("matchMedia", vi.fn((query: string) => ({
    matches: query === "(prefers-reduced-motion: reduce)",
    media: query,
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })));
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
});

test("reader keeps publication markup in a privilege-free sandbox and fetches only authorized opaque resources", async () => {
  const requestedResources: string[] = [];
  installReaderHandlers();
  server.use(
    http.get(`${apiOrigin}/api/v1/items/:itemId/resources/:resourceId`, ({ request }) => {
      requestedResources.push(new URL(request.url).pathname);
      return new HttpResponse("<html><body><h1>Secret chapter markup</h1></body></html>", {
        headers: { "Content-Type": "application/xhtml+xml" },
      });
    }),
  );

  renderApp(`/libraries/${libraryId}/items/${itemId}/read`);

  const frame = await screen.findByTitle("Safe Book reading content");
  expect(frame).toHaveAttribute("sandbox", "");
  const sandbox = frame.getAttribute("sandbox") ?? "";
  expect(sandbox).not.toMatch(/allow-(?:scripts|forms|popups|top-navigation|same-origin)/);
  expect(frame).toHaveAttribute("src", "blob:http://localhost/reader-1");
  expect(document.body).not.toHaveTextContent("Secret chapter markup");
  expect(createdObjectUrls).toHaveLength(1);
  expect(requestedResources).toEqual([firstHref]);
  expect((await axe.run(document.body, { iframes: false })).violations).toEqual([]);
});

test("reader rejects a manifest link outside the authorized Item resource route before fetching it", async () => {
  const externalRequests: string[] = [];
  installReaderHandlers({
    manifest: {
      ...manifest,
      readingOrder: [{ href: "https://attacker.example/chapter.xhtml", type: "application/xhtml+xml" }],
    },
  });
  server.use(http.get("https://attacker.example/*", ({ request }) => {
    externalRequests.push(request.url);
    return new HttpResponse("unsafe");
  }));

  renderApp(`/libraries/${libraryId}/items/${itemId}/read`);

  expect(await screen.findByRole("alert")).toHaveTextContent("This book cannot be opened safely");
  expect(externalRequests).toEqual([]);
  expect(screen.queryByTitle(/reading content/i)).not.toBeInTheDocument();
});

test("keyboard TOC navigation uses opaque links, revokes old object URLs, and returns focus", async () => {
  installReaderHandlers();
  const user = userEvent.setup();
  const view = renderApp(`/libraries/${libraryId}/items/${itemId}/read`);
  await screen.findByTitle("Safe Book reading content");

  const opener = screen.getByRole("button", { name: "Table of contents" });
  opener.focus();
  await user.keyboard("{Enter}");
  const dialog = screen.getByRole("dialog", { name: "Table of contents" });
  expect(within(dialog).getByRole("button", { name: "Close table of contents" })).toHaveFocus();

  within(dialog).getByRole("button", { name: "Chapter two" }).focus();
  await user.keyboard("{Enter}");
  await waitFor(() => { expect(screen.getByTitle("Safe Book reading content")).toHaveAttribute(
    "src",
    "blob:http://localhost/reader-2",
  ); });
  expect(screen.getByText("Chapter two")).toBeInTheDocument();
  expect(revokedObjectUrls).toContain("blob:http://localhost/reader-1");

  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog", { name: "Table of contents" })).not.toBeInTheDocument();
  expect(opener).toHaveFocus();

  view.unmount();
  expect(revokedObjectUrls).toContain("blob:http://localhost/reader-2");
});

test("reading settings honor reduced motion and persist font scaling and flow preference", async () => {
  installReaderHandlers();
  const user = userEvent.setup();
  renderApp(`/libraries/${libraryId}/items/${itemId}/read`);
  const frame = await screen.findByTitle("Safe Book reading content", {}, { timeout: 3_000 });
  expect(frame).toHaveAttribute("data-reduced-motion", "true");
  expect(frame).toHaveAttribute("data-reading-flow", "paginated");

  await user.selectOptions(screen.getByRole("combobox", { name: "Reading flow" }), "continuous");
  fireEvent.change(screen.getByRole("spinbutton", { name: "Font size" }), { target: { value: "150" } });

  expect(frame).toHaveAttribute("data-reading-flow", "continuous");
  expect(frame).toHaveAttribute("data-font-scale", "150");
  expect(JSON.parse(localStorage.getItem("folioharbor.reader.settings.v1") ?? "null")).toEqual({
    fontScale: 150,
    flow: "continuous",
  });
});

test("reader shows loading, request failure, and access-revoked states", async () => {
  let resolveManifest: (() => void) | undefined;
  const manifestPending = new Promise<void>((resolve) => { resolveManifest = resolve; });
  installReaderHandlers();
  server.use(http.get(`${apiOrigin}/api/v1/items/:itemId/manifest`, async () => {
    await manifestPending;
    return HttpResponse.json(manifest);
  }));
  const loadingView = renderApp(`/libraries/${libraryId}/items/${itemId}/read`);
  expect(await screen.findByText("Loading reader…")).toHaveAttribute("role", "status");
  await act(async () => {
    resolveManifest?.();
    await Promise.resolve();
  });
  await screen.findByTitle("Safe Book reading content");
  loadingView.unmount();

  installReaderHandlers();
  server.use(http.get(`${apiOrigin}/api/v1/items/:itemId/manifest`, () => problem(503, "service_unavailable")));
  const errorView = renderApp(`/libraries/${libraryId}/items/${itemId}/read`);
  expect(await screen.findByRole("alert")).toHaveTextContent("The reader could not be loaded");
  errorView.unmount();

  installReaderHandlers({ resourceStatus: 404 });
  renderApp(`/libraries/${libraryId}/items/${itemId}/read`);
  expect(await screen.findByRole("alert")).toHaveTextContent("Your access to this book is no longer available");
});

test("Readium Locator data is locale-independent and never contains DOM identity", async () => {
  const english = createLocator({
    href: `${secondHref}#middle`,
    mediaType: "application/xhtml+xml",
    position: 2,
    totalProgression: 0.5,
  });
  await i18n.changeLanguage("zh-CN");
  const chinese = createLocator({
    href: `${secondHref}#middle`,
    mediaType: "application/xhtml+xml",
    position: 2,
    totalProgression: 0.5,
  });

  expect(chinese).toEqual(english);
  expect(chinese).toEqual({
    href: `${secondHref}#middle`,
    type: "application/xhtml+xml",
    locations: { progression: 0, position: 2, totalProgression: 0.5 },
    extensions: { version: 1, values: {} },
  });
  expect(JSON.stringify(chinese)).not.toMatch(/node|selector|xpath/i);
});

test("saved cross-device progress opens its opaque resource without deriving a DOM position", async () => {
  const requestedResources: string[] = [];
  installReaderHandlers();
  server.use(
    http.get(`${apiOrigin}/api/v1/manifestations/:manifestationId/progress`, () => HttpResponse.json({
      manifestationId,
      locator: createLocator({
        href: `${secondHref}#middle`,
        mediaType: "application/xhtml+xml",
        position: 2,
        totalProgression: 0.5,
      }),
      version: 4,
      updatedAt: "2026-08-07T00:00:04Z",
    })),
    http.get(`${apiOrigin}/api/v1/items/:itemId/resources/:resourceId`, ({ request }) => {
      requestedResources.push(new URL(request.url).pathname);
      return new HttpResponse("<html><body>saved</body></html>", {
        headers: { "Content-Type": "application/xhtml+xml" },
      });
    }),
  );

  renderApp(`/libraries/${libraryId}/items/${itemId}/read`);

  await waitFor(() => { expect(requestedResources.at(-1)).toBe(secondHref); });
  expect(await screen.findByText("Progress saved on this account.")).toBeInTheDocument();
});

test("stale progress displays explicit global/device choices and visibility flush uses a bounded new mutation", async () => {
  installReaderHandlers();
  document.cookie = "folioharbor_csrf=reader-csrf; Path=/";
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
  const requests: { body: { baseVersion: number; clientMutationId: string; locator: ReturnType<typeof createLocator> }; keepalive: boolean }[] = [];
  server.use(
    http.get(`${apiOrigin}/api/v1/manifestations/:manifestationId/progress`, () => HttpResponse.json({
      manifestationId,
      locator: createLocator({ href: firstHref, mediaType: "application/xhtml+xml", position: 1, totalProgression: 0.8 }),
      version: 1,
      updatedAt: "2026-08-07T00:00:01Z",
    })),
    http.put(`${apiOrigin}/api/v1/manifestations/:manifestationId/progress`, async ({ request }) => {
      const body = await request.json() as { baseVersion: number; clientMutationId: string; locator: ReturnType<typeof createLocator> };
      requests.push({ body, keepalive: request.keepalive });
      if (requests.length === 1) {
        return HttpResponse.json({
          type: "https://folioharbor.test/problems/progress-conflict",
          title: "Reading progress conflict",
          status: 409,
          detail: "The position changed.",
          instance: "/problems/conflict-request",
          code: "progress_conflict",
          request_id: "conflict-request",
          global: {
            manifestationId,
            locator: createLocator({ href: firstHref, mediaType: "application/xhtml+xml", position: 1, totalProgression: 0.8 }),
            version: 2,
            updatedAt: "2026-08-07T00:00:02Z",
          },
          device: {
            deviceId: "018f47b5-58b4-7ba6-9a3a-d9f41f17e002",
            locator: body.locator,
            updatedAt: "2026-08-07T00:00:03Z",
          },
        }, { status: 409, headers: { "Content-Type": "application/problem+json" } });
      }
      return HttpResponse.json({
        manifestationId,
        locator: body.locator,
        version: 3,
        updatedAt: "2026-08-07T00:00:04Z",
      });
    }),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${libraryId}/items/${itemId}/read`);
  await screen.findByTitle("Safe Book reading content");
  await user.click(screen.getByRole("button", { name: "Table of contents" }));
  await user.click(screen.getByRole("button", { name: "Chapter two" }));
  await waitFor(() => {
    expect(screen.getByTitle("Safe Book reading content")).toHaveAttribute(
      "src",
      "blob:http://localhost/reader-2",
    );
  });
  document.dispatchEvent(new Event("visibilitychange"));

  const conflict = await screen.findByRole("dialog", { name: "Reading progress conflict" });
  expect(within(conflict).getByText("Account position: 80%")).toBeInTheDocument();
  expect(within(conflict).getByText("This device position: 50%")).toBeInTheDocument();
  expect(within(conflict).getByRole("button", { name: "Use account position" })).toHaveFocus();
  expect(requests[0]).toMatchObject({ keepalive: true, body: { baseVersion: 1 } });

  await user.click(within(conflict).getByRole("button", { name: "Use this device position" }));
  await screen.findByText("Progress saved on this account.");
  expect(requests).toHaveLength(2);
  expect(requests[1]?.body).toMatchObject({ baseVersion: 2, locator: { href: `${secondHref}#middle` } });
  expect(requests[1]?.body.clientMutationId).not.toBe(requests[0]?.body.clientMutationId);
});
