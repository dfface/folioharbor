import { randomBytes } from "node:crypto";

import { expect, test, type BrowserContext, type Page } from "@playwright/test";

import { generatedEpub, uniqueEmail, waitForMailToken } from "./support";

test.describe.configure({ timeout: 240_000 });

function userPassword(): string {
  return `E2E-${randomBytes(24).toString("base64url")}aA1!`;
}

async function registerVerifyAndLogin(page: Page, email: string, password: string): Promise<string> {
  await page.goto("/register");
  await page.getByLabel("Email", { exact: true }).fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("status")).toContainText("Verification required");

  const token = await waitForMailToken(email, "verify-email");
  await page.goto(`/verify-email?token=${encodeURIComponent(token)}`);
  await page.getByRole("button", { name: "Verify email" }).click();
  await expect(page.getByRole("status")).toContainText("Email verified");
  await page.getByRole("link", { name: "Log in" }).click();
  await page.getByLabel("Email", { exact: true }).fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page.getByRole("navigation", { name: "Account" })).toBeVisible();
  return token;
}

async function login(page: Page, email: string, password: string): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("Email", { exact: true }).fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page.getByRole("navigation", { name: "Account" })).toBeVisible();
}

async function downloadedBytes(context: BrowserContext, page: Page, byteLength: number): Promise<{
  bytes: Buffer;
  contentRange: string | undefined;
  status: number;
}> {
  await context.setExtraHTTPHeaders({ Range: `bytes=0-${String(byteLength - 1)}` });
  const [response, download] = await Promise.all([
    page.waitForResponse((candidate) => candidate.url().includes("/download"), { timeout: 15_000 }),
    page.waitForEvent("download", { timeout: 15_000 }),
    page.getByRole("link", { name: "Download EPUB" }).click(),
  ]);
  const stream = await download.createReadStream();
  const chunks: Uint8Array[] = [];
  for await (const chunk of stream) {
    if (!(chunk instanceof Uint8Array)) {
      throw new Error("browser Range download emitted an unexpected chunk type");
    }
    chunks.push(chunk);
  }
  return {
    bytes: Buffer.concat(chunks),
    contentRange: response.headers()["content-range"],
    status: response.status(),
  };
}

test("two real browser devices complete the Alice and Bob EPUB journey", async ({ browser }) => {
  const aliceContext = await browser.newContext();
  const bobDeviceAContext = await browser.newContext();
  const bobDeviceBContext = await browser.newContext();
  const alice = await aliceContext.newPage();
  const bobDeviceA = await bobDeviceAContext.newPage();
  const bobDeviceB = await bobDeviceBContext.newPage();
  const aliceEmail = uniqueEmail("browser-alice");
  const bobEmail = uniqueEmail("browser-bob");
  const alicePassword = userPassword();
  const bobPassword = userPassword();
  const title = "Browser Acceptance Book";
  const epub = generatedEpub(title);

  try {
    await registerVerifyAndLogin(alice, aliceEmail, alicePassword);
    await expect(alice).toHaveURL(/\/libraries\/[^/]+\/books$/u);
    const aliceLibraryId = /\/libraries\/([^/]+)/u.exec(alice.url())?.[1];
    if (aliceLibraryId === undefined) {
      throw new Error("Alice's browser did not enter her personal library");
    }
    await alice.getByRole("link", { name: "Members" }).click();
    await alice.getByLabel("Invitee email").fill(bobEmail);
    await alice.getByLabel("Invitation role").selectOption("reader");
    await alice.getByRole("button", { name: "Send invitation" }).click();
    await expect(alice.getByRole("status")).toHaveText("Invitation sent.");
    const invitationToken = await waitForMailToken(bobEmail, "accept-invitation");

    await registerVerifyAndLogin(bobDeviceA, bobEmail, bobPassword);
    await expect(bobDeviceA.getByLabel("Current library").locator("option")).toHaveCount(1);
    await bobDeviceA.goto(`/invitations/${encodeURIComponent(invitationToken)}`);
    await bobDeviceA.getByRole("button", { name: "Accept invitation" }).click();
    await expect(bobDeviceA).toHaveURL(new RegExp(`/libraries/${aliceLibraryId}/books$`, "u"));
    await expect(bobDeviceA.getByLabel("Current library").locator("option")).toHaveCount(2);

    await alice.getByRole("link", { name: "Uploads" }).click();
    await alice.getByLabel("EPUB file").setInputFiles({
      buffer: epub,
      mimeType: "application/epub+zip",
      name: "browser-acceptance.epub",
    });
    await alice.getByRole("button", { name: "Upload EPUB" }).click();
    await expect(alice.getByText("Book is ready.")).toBeVisible({ timeout: 30_000 });
    await alice.getByRole("link", { name: "Open book" }).click();
    const itemId = /\/items\/([^/]+)$/u.exec(alice.url())?.[1];
    if (itemId === undefined) {
      throw new Error("Alice's browser upload did not navigate to an Item");
    }

    await bobDeviceA.reload();
    await bobDeviceA.getByRole("link", { name: title }).click();
    await expect(bobDeviceA.getByText("Online reading only")).toBeVisible();
    await expect(bobDeviceA.getByRole("link", { name: "Download EPUB" })).toHaveCount(0);
    await bobDeviceA.getByRole("link", { name: "Read online" }).click();
    const readerFrameA = bobDeviceA.frameLocator(`iframe[title="${title} reading content"]`);
    await expect(readerFrameA.getByRole("heading", { name: title })).toBeVisible();
    await bobDeviceA.getByRole("button", { name: "Next section" }).click();
    await expect(readerFrameA.getByRole("heading", { name: `${title} — Chapter Two` })).toBeVisible();
    await expect(bobDeviceA.getByRole("status")).toContainText("Progress saved", { timeout: 10_000 });

    await login(bobDeviceB, bobEmail, bobPassword);
    await bobDeviceB.getByLabel("Current library").selectOption(aliceLibraryId);
    await bobDeviceB.getByRole("link", { name: title }).click();
    await bobDeviceB.getByRole("link", { name: "Read online" }).click();
    const readerFrameB = bobDeviceB.frameLocator(`iframe[title="${title} reading content"]`);
    await expect(readerFrameB.getByRole("heading", { name: `${title} — Chapter Two` })).toBeVisible();

    await alice.getByRole("link", { name: "Library settings" }).click();
    await alice.getByLabel("Allow readers to download original EPUB files").check();
    await alice.getByRole("button", { name: "Save library settings" }).click();
    await expect(alice.getByRole("status")).toHaveText("Library settings saved.");

    await bobDeviceB.reload();
    await bobDeviceB.getByRole("link", { name: "All books" }).click();
    await bobDeviceB.getByRole("link", { name: title }).click();
    await expect(bobDeviceB.getByRole("link", { name: "Download EPUB" })).toBeVisible();
    const downloaded = await downloadedBytes(bobDeviceBContext, bobDeviceB, epub.byteLength);
    expect(downloaded.status).toBe(206);
    expect(downloaded.contentRange).toBe(
      `bytes 0-${String(epub.byteLength - 1)}/${String(epub.byteLength)}`,
    );
    expect(
      downloaded.bytes.equals(epub),
      "browser Range download bytes did not match the uploaded EPUB",
    ).toBe(true);
  } finally {
    await Promise.allSettled([
      aliceContext.close(),
      bobDeviceAContext.close(),
      bobDeviceBContext.close(),
    ]);
  }
});
