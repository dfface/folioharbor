import { expect, test, type BrowserContext, type Page, type Route } from "@playwright/test";

const libraryId = "018f47b5-58b4-7ba6-9a3a-d9f41f17b001";
const itemId = "018f47b5-58b4-7ba6-9a3a-d9f41f17c001";
const manifestationId = "018f47b5-58b4-7ba6-9a3a-d9f41f17d001";
const accountA = "018f47b5-58b4-7ba6-9a3a-d9f41f17a101";
const accountB = "018f47b5-58b4-7ba6-9a3a-d9f41f17a102";
const readerPath = `/libraries/${libraryId}/items/${itemId}/read`;
const chapterHrefs = ["chapter-one", "chapter-two", "chapter-three"].map(
  (chapter) => `/api/v1/items/${itemId}/resources/${chapter}`,
);

interface Locator {
  href: string;
  type: string;
  locations: { position: number; progression: number; totalProgression: number };
  extensions: { version: 1; values: Record<string, never> };
}

interface ReadingProgress {
  manifestationId: string;
  locator: Locator;
  version: number;
  updatedAt: string;
}

interface ProgressUpdate {
  accountId: string;
  baseVersion: number;
  clientMutationId: string;
  deviceId: string;
  locator: Locator;
}

interface SharedAccount {
  denyResources: boolean;
  failProgressReads: number;
  failProgressWrites: boolean;
  global: ReadingProgress | null;
  progressReads: number;
  resourceRequests: string[];
  userId: string;
  writes: ProgressUpdate[];
}

function locator(chapter: number, totalProgression: number): Locator {
  return {
    href: chapterHrefs[chapter - 1] ?? chapterHrefs[0] ?? "",
    type: "application/xhtml+xml",
    locations: { position: chapter, progression: 0, totalProgression },
    extensions: { version: 1, values: {} },
  };
}

function account(overrides: Partial<SharedAccount> = {}): SharedAccount {
  return {
    denyResources: false,
    failProgressReads: 0,
    failProgressWrites: false,
    global: null,
    progressReads: 0,
    resourceRequests: [],
    userId: accountA,
    writes: [],
    ...overrides,
  };
}

function publication(chapter: string): string {
  const paragraphs = Array.from(
    { length: 80 },
    (_, index) => `<p>Page ${String(index + 1)} — ${"sandboxed reader content ".repeat(8)}</p>`,
  ).join("");
  return `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>${chapter}</title></head>
<body><h1>Browser ${chapter}</h1>${paragraphs}</body></html>`;
}

async function json(route: Route, body: unknown, status = 200): Promise<void> {
  await route.fulfill({ body: JSON.stringify(body), contentType: "application/json", status });
}

async function problem(route: Route, status: number, code: string): Promise<void> {
  await route.fulfill({
    body: JSON.stringify({
      type: `https://folioharbor.test/problems/${code}`,
      title: "Request failed",
      status,
      detail: "The request could not be completed.",
      instance: "/problems/browser-request",
      code,
      request_id: "browser-request",
    }),
    contentType: "application/problem+json",
    status,
  });
}

async function installReaderApi(context: BrowserContext, state: SharedAccount): Promise<void> {
  await context.addCookies([{
    name: "folioharbor_csrf",
    value: "browser-csrf",
    domain: "127.0.0.1",
    path: "/",
    sameSite: "Lax",
  }]);
  await context.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/v1/auth/session") {
      await json(route, {
        user_id: state.userId,
        session_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e",
        is_current: true,
        status: "active",
      });
      return;
    }
    if (path === "/api/v1/libraries") {
      await json(route, [{
        library_id: libraryId,
        name: "Browser Library",
        role: "owner",
        is_personal: true,
        reader_download_enabled: false,
        capabilities: {
          can_view_catalog: true,
          can_upload: true,
          can_invite_members: true,
          can_manage_members: true,
          can_manage_settings: true,
        },
      }]);
      return;
    }
    if (path === `/api/v1/libraries/${libraryId}`) {
      await json(route, {
        library_id: libraryId,
        name: "Browser Library",
        role: "owner",
        is_personal: true,
        reader_download_enabled: false,
        capabilities: {
          can_view_catalog: true,
          can_upload: true,
          can_invite_members: true,
          can_manage_members: true,
          can_manage_settings: true,
        },
      });
      return;
    }
    if (path === `/api/v1/items/${itemId}/manifest`) {
      await json(route, {
        metadata: { title: "Browser Book", authors: ["Writer"], languages: ["en"] },
        manifestationId,
        readingOrder: chapterHrefs.map((href) => ({ href, type: "application/xhtml+xml" })),
        resources: [],
        toc: chapterHrefs.map((href, index) => ({
          href: `${href}#start`,
          type: "application/xhtml+xml",
          title: `Chapter ${String(index + 1)}`,
        })),
        links: [{
          href: `/api/v1/items/${itemId}/manifest`,
          type: "application/webpub+json",
          rel: "self",
        }],
      });
      return;
    }
    if (chapterHrefs.includes(path)) {
      state.resourceRequests.push(path);
      if (state.denyResources) {
        await problem(route, 403, "reader_access_denied");
        return;
      }
      await route.fulfill({
        body: publication(path.slice(path.lastIndexOf("/") + 1)),
        contentType: "application/xhtml+xml",
        status: 200,
      });
      return;
    }
    if (path === `/api/v1/manifestations/${manifestationId}/progress` && request.method() === "GET") {
      state.progressReads += 1;
      if (state.failProgressReads > 0) {
        state.failProgressReads -= 1;
        await problem(route, 503, "progress_unavailable");
      } else if (state.global === null) {
        await route.fulfill({ status: 204 });
      } else {
        await json(route, state.global);
      }
      return;
    }
    if (path === `/api/v1/manifestations/${manifestationId}/progress` && request.method() === "PUT") {
      const update = JSON.parse(request.postData() ?? "null") as ProgressUpdate;
      state.writes.push(structuredClone(update));
      if (update.accountId !== state.userId) {
        await problem(route, 403, "progress_account_mismatch");
        return;
      }
      if (state.failProgressWrites) {
        await problem(route, 503, "progress_unavailable");
        return;
      }
      const version = state.global?.version ?? 0;
      if (update.baseVersion !== version) {
        await route.fulfill({
          body: JSON.stringify({
            type: "https://folioharbor.test/problems/progress-conflict",
            title: "Reading progress conflict",
            status: 409,
            detail: "The position changed.",
            instance: "/problems/browser-conflict",
            code: "progress_conflict",
            request_id: "browser-conflict",
            global: state.global ?? {
              manifestationId,
              locator: null,
              version: 0,
              updatedAt: null,
            },
            device: {
              deviceId: update.deviceId,
              locator: update.locator,
              updatedAt: "2026-08-07T00:00:09Z",
            },
          }),
          contentType: "application/problem+json",
          status: 409,
        });
        return;
      }
      state.global = {
        manifestationId,
        locator: update.locator,
        version: version + 1,
        updatedAt: "2026-08-07T00:00:10Z",
      };
      await json(route, state.global);
      return;
    }
    await problem(route, 404, "not_found");
  });
}

async function openReader(page: Page): Promise<void> {
  await page.goto(readerPath);
  await expect(page.locator("iframe[title='Browser Book reading content']")).toBeVisible();
}

async function chooseChapter(page: Page, chapter: number): Promise<void> {
  await page.getByRole("button", { name: "Table of contents" }).click();
  await page.getByRole("dialog", { name: "Table of contents" })
    .getByRole("button", { name: `Chapter ${String(chapter)}`, exact: true })
    .click();
  await page.keyboard.press("Escape");
}

async function frameMetrics(page: Page) {
  return page.frameLocator("iframe").locator("body").evaluate((body) => {
    const root = body.ownerDocument.documentElement;
    const style = getComputedStyle(body);
    const rootStyle = getComputedStyle(root);
    return {
      bodyClientWidth: body.clientWidth,
      bodyScrollWidth: body.scrollWidth,
      columnWidth: style.columnWidth,
      fontSize: style.fontSize,
      overflowX: style.overflowX,
      rootClientHeight: root.clientHeight,
      rootOverflowY: rootStyle.overflowY,
      rootScrollHeight: root.scrollHeight,
    };
  });
}

test("real browser enforces frame isolation, modal focus, layout modes, revocation, and access loss", async ({ browser }) => {
  const state = account();
  const context = await browser.newContext();
  await installReaderApi(context, state);
  const page = await context.newPage();
  await openReader(page);

  const iframe = page.locator("iframe[title='Browser Book reading content']");
  await expect(iframe).toHaveAttribute("sandbox", "");
  await expect(iframe).toHaveAttribute("src", /^blob:/);
  await expect(page.getByText("Browser chapter-one")).toHaveCount(0);
  await expect(page.frameLocator("iframe").getByRole("heading", { name: "Browser chapter-one" })).toBeVisible();

  const paginated = await frameMetrics(page);
  expect(paginated.columnWidth).not.toBe("auto");
  expect(paginated.overflowX).toBe("auto");
  expect(paginated.bodyScrollWidth).toBeGreaterThan(paginated.bodyClientWidth);

  const tocButton = page.getByRole("button", { name: "Table of contents" });
  await tocButton.click();
  const dialog = page.getByRole("dialog", { name: "Table of contents" });
  const close = dialog.getByRole("button", { name: "Close table of contents" });
  await expect(close).toBeFocused();
  const modalSource = await iframe.getAttribute("src");
  await page.keyboard.press("ArrowRight");
  await expect(iframe).toHaveAttribute("src", modalSource ?? "");
  await page.keyboard.press("Shift+Tab");
  await expect(dialog.getByRole("button", { name: "Chapter 3" })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(close).toBeFocused();
  await expect(page.locator("[inert]")).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(tocButton).toBeFocused();

  await page.getByRole("combobox", { name: "Reading flow" }).selectOption("continuous");
  await expect(iframe).toHaveAttribute("data-reading-flow", "continuous");
  await page.getByRole("spinbutton", { name: "Font size" }).fill("150");
  await expect(iframe).toHaveAttribute("data-font-scale", "150");
  const continuous = await frameMetrics(page);
  expect(continuous.columnWidth).toBe("auto");
  expect(continuous.rootOverflowY).toBe("auto");
  expect(continuous.rootScrollHeight).toBeGreaterThan(continuous.rootClientHeight);
  expect(Number.parseFloat(continuous.fontSize)).toBeGreaterThan(20);

  const oldSource = await iframe.getAttribute("src");
  if (oldSource === null) {
    throw new Error("reader frame is missing its Blob URL");
  }
  await chooseChapter(page, 2);
  await expect(iframe).not.toHaveAttribute("src", oldSource);
  expect(await page.evaluate(async (url) => {
    try {
      return (await fetch(url)).ok;
    } catch {
      return false;
    }
  }, oldSource)).toBe(false);
  expect(state.resourceRequests).toEqual([chapterHrefs[0], chapterHrefs[1]]);

  state.denyResources = true;
  await page.getByRole("button", { name: "Next section" }).click();
  await expect(page.getByRole("alert")).toContainText("no longer available");
  await expect(page.locator("iframe")).toHaveCount(0);
  await context.close();
});

test("two isolated browser contexts sharing one account surface and explicitly resolve a stale conflict", async ({ browser }) => {
  const state = account();
  const contextA = await browser.newContext();
  const contextB = await browser.newContext();
  await Promise.all([installReaderApi(contextA, state), installReaderApi(contextB, state)]);
  const pageA = await contextA.newPage();
  const pageB = await contextB.newPage();
  await Promise.all([openReader(pageA), openReader(pageB)]);
  await Promise.all([
    expect(pageA.getByText("No reading progress is saved yet.")).toBeVisible(),
    expect(pageB.getByText("No reading progress is saved yet.")).toBeVisible(),
  ]);
  const [deviceA, deviceB] = await Promise.all([
    pageA.evaluate(() => localStorage.getItem("folioharbor.reader.device-id.v1")),
    pageB.evaluate(() => localStorage.getItem("folioharbor.reader.device-id.v1")),
  ]);
  expect(deviceA).not.toBe(deviceB);

  await chooseChapter(pageA, 2);
  await expect(pageA.getByText("Progress saved on this account.")).toBeVisible();
  await pageB.getByRole("button", { name: "Table of contents" }).click();
  await pageB.getByRole("dialog", { name: "Table of contents" })
    .getByRole("button", { name: "Chapter 3" })
    .click();
  const conflict = pageB.getByRole("dialog", { name: "Reading progress conflict" });
  await expect(conflict).toBeVisible();
  await expect(conflict.getByText("Account position: 33%")).toBeVisible();
  await expect(conflict.getByText("This device position: 67%")).toBeVisible();
  await conflict.getByRole("button", { name: "Use this device position" }).click();
  await expect(pageB.getByText("Progress saved on this account.")).toBeVisible();
  expect(state.global).toMatchObject({ version: 2, locator: { href: `${chapterHrefs[2] ?? ""}#start` } });

  await Promise.all([contextA.close(), contextB.close()]);
});

test("same-install account replacement cannot replay the prior account's pending Locator", async ({ browser }) => {
  const state = account({ failProgressWrites: true });
  const context = await browser.newContext();
  await installReaderApi(context, state);
  const page = await context.newPage();
  await openReader(page);
  await chooseChapter(page, 2);
  await expect(page.getByText(/Offline/)).toBeVisible();
  expect(state.writes).toHaveLength(1);

  state.userId = accountB;
  state.failProgressWrites = false;
  state.global = null;
  await page.reload();
  await expect(page.getByText("No reading progress is saved yet.")).toBeVisible();
  await expect(page.locator("iframe")).toBeVisible();
  expect(state.writes).toHaveLength(1);
  const keys = await page.evaluate(() => Object.keys(localStorage));
  expect(keys.some((key) => key.includes(accountA))).toBe(true);
  expect(keys.some((key) => key.includes(accountB) && key.includes("progress"))).toBe(false);
  await context.close();
});

test("online recovery repeats a failed initial progress read and loads the saved Locator", async ({ browser }) => {
  const state = account({
    failProgressReads: 1,
    global: {
      manifestationId,
      locator: locator(3, 2 / 3),
      version: 4,
      updatedAt: "2026-08-07T00:00:04Z",
    },
  });
  const context = await browser.newContext();
  await installReaderApi(context, state);
  const page = await context.newPage();
  await openReader(page);
  await expect(page.getByText(/Offline/)).toBeVisible();
  expect(state.progressReads).toBe(1);

  await page.evaluate(() => { window.dispatchEvent(new Event("online")); });
  await expect(page.getByText("Progress saved on this account.")).toBeVisible();
  await expect(page.frameLocator("iframe").getByRole("heading", { name: "Browser chapter-three" })).toBeVisible();
  expect(state.progressReads).toBe(2);
  await context.close();
});
