import { createHash, randomUUID } from "node:crypto";

import {
  expect,
  request as playwrightRequest,
  type APIRequestContext,
  type APIResponse,
} from "@playwright/test";

const apiBaseUrl = process.env.FOLIOHARBOR_E2E_API_URL ?? "http://127.0.0.1:3000";
const mailBaseUrl = process.env.FOLIOHARBOR_E2E_MAIL_URL ?? "http://127.0.0.1:8025";
const password = "Valid e2e password 2026!";

export interface LibraryView {
  library_id: string;
  name: string;
  role: "owner" | "editor" | "reader";
  reader_download_enabled: boolean;
  capabilities: {
    can_upload: boolean;
    can_invite_members: boolean;
    can_manage_members: boolean;
    can_manage_settings: boolean;
  };
}

export interface SessionClient {
  api: APIRequestContext;
  email: string;
  userId: string;
  cookieHeader: string;
  csrfToken: string;
}

export interface CollaborativePair {
  alice: SessionClient;
  aliceLibrary: LibraryView;
  bob: SessionClient;
  bobPersonalLibrary: LibraryView;
}

export interface UploadView {
  upload_id: string;
  library_id: string;
  state: string;
  error_code: string | null;
  item_id: string | null;
}

interface MailSummary {
  ID: string;
}

function object(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null
    ? value as Record<string, unknown>
    : null;
}

export async function responseJson(response: APIResponse): Promise<unknown> {
  return response.json() as Promise<unknown>;
}

export async function expectStatus(response: APIResponse, status: number): Promise<void> {
  const body = await response.text();
  expect(response.status(), body).toBe(status);
}

function cookie(response: APIResponse, name: string): string {
  for (const header of response.headersArray()) {
    if (header.name.toLowerCase() !== "set-cookie") {
      continue;
    }
    const [pair] = header.value.split(";", 1);
    const [cookieName, ...parts] = pair?.split("=") ?? [];
    if (cookieName === name) {
      return parts.join("=");
    }
  }
  throw new Error(`login did not issue ${name}`);
}

function mailSummaries(value: unknown): MailSummary[] {
  const root = object(value);
  if (root === null || !Array.isArray(root.messages)) {
    return [];
  }
  return root.messages.flatMap((candidate) => {
    const row = object(candidate);
    return row !== null && typeof row.ID === "string" ? [{ ID: row.ID }] : [];
  });
}

function tokenFromMessage(value: unknown, expectedPath: string, email: string): string | null {
  const row = object(value);
  if (row === null || !JSON.stringify(row).toLowerCase().includes(email.toLowerCase())) {
    return null;
  }
  for (const field of ["Text", "HTML", "text", "html"] as const) {
    const content = row[field];
    if (typeof content !== "string" || !content.includes(expectedPath)) {
      continue;
    }
    const match = /[?&]token=([^\s<"&]+)/u.exec(content);
    if (match?.[1] !== undefined) {
      return decodeURIComponent(match[1]);
    }
  }
  return null;
}

export function uniqueEmail(label: string): string {
  return `${label}-${String(Date.now())}-${randomUUID()}@e2e.invalid`;
}

export async function anonymousApi(): Promise<APIRequestContext> {
  return playwrightRequest.newContext({ baseURL: apiBaseUrl });
}

export async function readProblemCode(response: APIResponse): Promise<string | null> {
  const payload = object(await responseJson(response));
  return payload !== null && typeof payload.code === "string" ? payload.code : null;
}

export async function waitForMailToken(email: string, path: string): Promise<string> {
  const mail = await playwrightRequest.newContext({ baseURL: mailBaseUrl });
  try {
    for (let attempt = 0; attempt < 80; attempt += 1) {
      const listResponse = await mail.get("/api/v1/messages");
      if (listResponse.ok()) {
        const summaries = mailSummaries(await responseJson(listResponse));
        for (const summary of summaries) {
          const messageResponse = await mail.get(`/api/v1/message/${encodeURIComponent(summary.ID)}`);
          if (!messageResponse.ok()) {
            continue;
          }
          const token = tokenFromMessage(await responseJson(messageResponse), path, email);
          if (token !== null) {
            return token;
          }
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  } finally {
    await mail.dispose();
  }
  throw new Error(`timed out waiting for ${path} mail to ${email}`);
}

export async function registerAndVerify(email: string): Promise<void> {
  const api = await anonymousApi();
  try {
    const registered = await api.post("/api/v1/auth/register", {
      data: { email, password },
    });
    await expectStatus(registered, 202);
    const token = await waitForMailToken(email, "verify-email");
    const verified = await api.post("/api/v1/auth/verify-email", { data: { token } });
    await expectStatus(verified, 204);
  } finally {
    await api.dispose();
  }
}

export async function login(email: string): Promise<SessionClient> {
  const anonymous = await anonymousApi();
  try {
    const response = await anonymous.post("/api/v1/auth/login", {
      data: { email, password },
    });
    await expectStatus(response, 200);
    const payload = object(await responseJson(response));
    if (payload === null || typeof payload.user_id !== "string") {
      throw new Error("login response did not contain a user identifier");
    }
    const session = cookie(response, "folioharbor_session");
    const csrf = cookie(response, "folioharbor_csrf");
    const api = await playwrightRequest.newContext({
      baseURL: apiBaseUrl,
      extraHTTPHeaders: {
        Cookie: `folioharbor_session=${session}; folioharbor_csrf=${csrf}`,
        "X-CSRF-Token": csrf,
      },
    });
    return {
      api,
      email,
      userId: payload.user_id,
      cookieHeader: `folioharbor_session=${session}; folioharbor_csrf=${csrf}`,
      csrfToken: csrf,
    };
  } finally {
    await anonymous.dispose();
  }
}

export async function libraries(client: SessionClient): Promise<LibraryView[]> {
  const response = await client.api.get("/api/v1/libraries");
  await expectStatus(response, 200);
  return await responseJson(response) as LibraryView[];
}

export async function createCollaborativePair(role: "reader" | "editor" = "reader"):
Promise<CollaborativePair> {
  const aliceEmail = uniqueEmail("alice");
  await registerAndVerify(aliceEmail);
  const alice = await login(aliceEmail);
  const aliceLibraries = await libraries(alice);
  expect(aliceLibraries).toHaveLength(1);
  const aliceLibrary = aliceLibraries[0];
  if (aliceLibrary === undefined) {
    throw new Error("Alice's personal library is missing");
  }

  const bobEmail = uniqueEmail("bob");
  const invited = await alice.api.post(
    `/api/v1/libraries/${aliceLibrary.library_id}/invitations`,
    { data: { email: bobEmail, role } },
  );
  await expectStatus(invited, 204);
  const invitationToken = await waitForMailToken(bobEmail, "accept-invitation");

  await registerAndVerify(bobEmail);
  const bob = await login(bobEmail);
  const beforeAcceptance = await libraries(bob);
  expect(beforeAcceptance).toHaveLength(1);
  const bobPersonalLibrary = beforeAcceptance[0];
  if (bobPersonalLibrary === undefined) {
    throw new Error("Bob's personal library is missing");
  }
  const accepted = await bob.api.post("/api/v1/invitations/accept", {
    data: { token: invitationToken },
  });
  await expectStatus(accepted, 200);
  expect(await responseJson(accepted)).toEqual({
    status: "accepted",
    library_id: aliceLibrary.library_id,
  });

  return { alice, aliceLibrary, bob, bobPersonalLibrary };
}

export async function uploadPublication(
  client: SessionClient,
  libraryId: string,
  bytes: Buffer,
  fileName = "generated.epub",
): Promise<UploadView> {
  const upload = await createUpload(client, libraryId, bytes.byteLength, fileName);
  const received = await client.api.put(
    `/api/v1/libraries/${libraryId}/uploads/${upload.upload_id}/content`,
    { data: bytes, headers: { "Content-Type": "application/epub+zip" } },
  );
  await expectStatus(received, 202);
  return waitForUpload(client, libraryId, upload.upload_id);
}

export async function createUpload(
  client: SessionClient,
  libraryId: string,
  declaredBytes: number,
  fileName = "generated.epub",
): Promise<UploadView> {
  const created = await client.api.post(`/api/v1/libraries/${libraryId}/uploads`, {
    data: {
      file_name: fileName,
      media_type: "application/epub+zip",
      declared_bytes: declaredBytes,
    },
  });
  await expectStatus(created, 202);
  return await responseJson(created) as UploadView;
}

export async function waitForUpload(
  client: SessionClient,
  libraryId: string,
  uploadId: string,
): Promise<UploadView> {
  let lastState = "unknown";
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const status = await client.api.get(
      `/api/v1/libraries/${libraryId}/uploads/${uploadId}`,
    );
    await expectStatus(status, 200);
    const current = await responseJson(status) as UploadView;
    lastState = current.state;
    if (["ready", "duplicate", "failed"].includes(current.state)) {
      return current;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`upload ${uploadId} did not reach a terminal state (last: ${lastState})`);
}

function crc32(bytes: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function u16(value: number): Buffer {
  const bytes = Buffer.alloc(2);
  bytes.writeUInt16LE(value);
  return bytes;
}

function u32(value: number): Buffer {
  const bytes = Buffer.alloc(4);
  bytes.writeUInt32LE(value);
  return bytes;
}

function zip(entries: readonly (readonly [string, string])[]): Buffer {
  const localParts: Buffer[] = [];
  const centralParts: Buffer[] = [];
  let offset = 0;
  for (const [name, value] of entries) {
    const nameBytes = Buffer.from(name);
    const data = Buffer.from(value);
    const checksum = crc32(data);
    const local = Buffer.concat([
      u32(0x04034b50), u16(20), u16(0), u16(0), u16(0), u16(0), u32(checksum),
      u32(data.length), u32(data.length), u16(nameBytes.length), u16(0), nameBytes, data,
    ]);
    localParts.push(local);
    centralParts.push(Buffer.concat([
      u32(0x02014b50), u16(20), u16(20), u16(0), u16(0), u16(0), u16(0),
      u32(checksum), u32(data.length), u32(data.length), u16(nameBytes.length),
      u16(0), u16(0), u16(0), u16(0), u32(0), u32(offset), nameBytes,
    ]));
    offset += local.length;
  }
  const central = Buffer.concat(centralParts);
  return Buffer.concat([
    ...localParts,
    central,
    u32(0x06054b50), u16(0), u16(0), u16(entries.length), u16(entries.length),
    u32(central.length), u32(offset), u16(0),
  ]);
}

export function generatedEpub(title = "Generated E2E Book", paddingBytes = 0): Buffer {
  const padding = "x".repeat(paddingBytes);
  return zip([
    ["mimetype", "application/epub+zip"],
    ["META-INF/container.xml", `<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>`],
    ["OEBPS/content.opf", `<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="book-id">urn:uuid:${randomUUID()}</dc:identifier>
    <dc:title>${title}</dc:title><dc:creator>E2E Author</dc:creator><dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="chapter"/></spine>
</package>`],
    ["OEBPS/nav.xhtml", `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Contents</title></head>
<body><nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>`],
    ["OEBPS/chapter.xhtml", `<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>${title}</title></head>
<body><h1>${title}</h1><p>The complete vertical slice is readable.${padding}</p></body></html>`],
  ]);
}

export function maliciousTraversalEpub(): Buffer {
  return zip([
    ["mimetype", "application/epub+zip"],
    ["../outside.xhtml", "<p>must never escape</p>"],
    ["META-INF/container.xml", `<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>`],
  ]);
}

export function sha256(bytes: Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export function newId(): string {
  return randomUUID();
}
