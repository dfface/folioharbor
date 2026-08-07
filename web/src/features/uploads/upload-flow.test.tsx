import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, expect, test } from "vitest";

import { renderApp } from "../../test/render";
import { server } from "../../test/server";

const apiOrigin = "*";
const libraryId = "018f47b5-58b4-7ba6-9a3a-d9f41f17b101";
const uploadId = "018f47b5-58b4-7ba6-9a3a-d9f41f17e101";
const itemId = "018f47b5-58b4-7ba6-9a3a-d9f41f17c101";
const nativeXMLHttpRequest = globalThis.XMLHttpRequest;

const ownerLibrary = {
  library_id: libraryId,
  name: "Upload Library",
  role: "owner",
  reader_download_enabled: false,
  capabilities: {
    can_upload: true,
    can_invite_members: true,
    can_manage_members: true,
    can_manage_settings: true,
  },
} as const;

function uploadStatus(
  state: "created" | "receiving" | "received" | "queued" | "validating" | "importing" | "retry_wait" | "ready" | "duplicate" | "failed" | "expired",
) {
  return {
    upload_id: uploadId,
    library_id: libraryId,
    file_name: "book.epub",
    media_type: "application/epub+zip",
    declared_bytes: 10,
    received_bytes: state === "created" ? 0 : 10,
    state,
    status_url: `/api/v1/libraries/${libraryId}/uploads/${uploadId}`,
    error_code: state === "failed" ? "invalid_epub" : null,
    item_id: state === "duplicate" || state === "ready" ? itemId : null,
  };
}

class FakeXMLHttpRequest extends EventTarget {
  static latest: FakeXMLHttpRequest | null = null;

  readonly upload = new EventTarget();
  response: unknown = null;
  status = 0;
  sentBody: Document | XMLHttpRequestBodyInit | null = null;
  aborted = false;

  constructor() {
    super();
    FakeXMLHttpRequest.latest = this;
  }

  open() {
    // The fake records only the transfer lifecycle used by this test.
  }

  setRequestHeader() {
    // Headers are covered by the HTTP adapter contract, not this progress fake.
  }

  send(body: Document | XMLHttpRequestBodyInit | null) {
    this.sentBody = body;
  }

  abort() {
    this.aborted = true;
    this.dispatchEvent(new Event("abort"));
  }

  progress(loaded: number, total: number) {
    this.upload.dispatchEvent(new ProgressEvent("progress", { lengthComputable: true, loaded, total }));
  }

  complete(response: unknown) {
    this.status = 202;
    this.response = response;
    this.dispatchEvent(new Event("load"));
  }
}

function authenticatedUploadHandlers() {
  server.use(
    http.get(`${apiOrigin}/api/v1/auth/session`, () =>
      HttpResponse.json({ session_id: "018f47b5-58b4-7ba6-9a3a-d9f41f17a26e", is_current: true, status: "active" }),
    ),
    http.get(`${apiOrigin}/api/v1/libraries`, () => HttpResponse.json([ownerLibrary])),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId`, () => HttpResponse.json(ownerLibrary)),
  );
}

beforeEach(() => {
  FakeXMLHttpRequest.latest = null;
  Object.defineProperty(globalThis, "XMLHttpRequest", {
    configurable: true,
    value: FakeXMLHttpRequest,
  });
});

afterEach(() => {
  Object.defineProperty(globalThis, "XMLHttpRequest", { configurable: true, value: nativeXMLHttpRequest });
});

test("upload progress uses transmitted bytes then switches to reload-safe background polling and Duplicate linking", async () => {
  authenticatedUploadHandlers();
  let statusRequests = 0;
  server.use(
    http.post(`${apiOrigin}/api/v1/libraries/:libraryId/uploads`, () =>
      HttpResponse.json(uploadStatus("created"), { status: 202 }),
    ),
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/uploads/:uploadId`, () => {
      statusRequests += 1;
      return HttpResponse.json(uploadStatus("duplicate"));
    }),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${libraryId}/uploads`);
  const file = new File(["0123456789"], "book.epub", { type: "application/epub+zip" });
  await user.upload(await screen.findByLabelText("EPUB file"), file);
  await user.click(screen.getByRole("button", { name: "Upload EPUB" }));

  const request = await waitForRequest();
  act(() => { request.progress(5, 10); });
  expect(screen.getByRole("progressbar", { name: "Upload progress" })).toHaveAttribute("aria-valuenow", "50");
  expect(screen.getByText("5 B of 10 B transmitted (50%)")).toBeInTheDocument();

  act(() => { request.complete(uploadStatus("queued")); });
  expect(await screen.findByText("Transfer complete. Processing continues in the background.")).toBeInTheDocument();
  expect(await screen.findByText("Already in this library.")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Open existing book" })).toHaveAttribute(
    "href",
    `/libraries/${libraryId}/items/${itemId}`,
  );
  expect(window.location.search).toBe(`?upload=${uploadId}`);
  expect(statusRequests).toBeGreaterThan(0);
});

test("a selected file over 1 GiB is rejected before creating a server upload", async () => {
  authenticatedUploadHandlers();
  let createRequests = 0;
  server.use(
    http.post(`${apiOrigin}/api/v1/libraries/:libraryId/uploads`, () => {
      createRequests += 1;
      return HttpResponse.json(uploadStatus("created"), { status: 202 });
    }),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${libraryId}/uploads`);
  const oversized = new File(["x"], "large.epub", { type: "application/epub+zip" });
  Object.defineProperty(oversized, "size", { value: 1_073_741_825 });
  await user.upload(await screen.findByLabelText("EPUB file"), oversized);
  await user.click(screen.getByRole("button", { name: "Upload EPUB" }));

  expect(await screen.findByRole("alert")).toHaveTextContent("EPUB files must be 1 GiB or smaller.");
  expect(createRequests).toBe(0);
});

test("an in-flight transfer can be canceled without leaving transport code in the page", async () => {
  authenticatedUploadHandlers();
  server.use(
    http.post(`${apiOrigin}/api/v1/libraries/:libraryId/uploads`, () =>
      HttpResponse.json(uploadStatus("created"), { status: 202 }),
    ),
  );

  const user = userEvent.setup();
  renderApp(`/libraries/${libraryId}/uploads`);
  await user.upload(
    await screen.findByLabelText("EPUB file"),
    new File(["content"], "book.epub", { type: "application/epub+zip" }),
  );
  await user.click(screen.getByRole("button", { name: "Upload EPUB" }));
  const request = await waitForRequest();
  await user.click(screen.getByRole("button", { name: "Cancel transfer" }));

  expect(request.aborted).toBe(true);
  expect(await screen.findByText("Upload canceled.")).toBeInTheDocument();
});

test.each([
  ["created", "Upload created."],
  ["receiving", "Receiving file."],
  ["received", "File received."],
  ["queued", "Waiting for background processing."],
  ["validating", "Validating EPUB."],
  ["importing", "Adding the book to the library."],
  ["retry_wait", "A temporary problem occurred. Retrying automatically."],
  ["ready", "Book is ready."],
  ["duplicate", "Already in this library."],
  ["failed", "This EPUB could not be imported. Choose a valid EPUB and try again."],
  ["expired", "This upload expired. Select the file to start again."],
] as const)("a reloaded %s upload renders durable status copy", async (state, copy) => {
  authenticatedUploadHandlers();
  server.use(
    http.get(`${apiOrigin}/api/v1/libraries/:libraryId/uploads/:uploadId`, () =>
      HttpResponse.json(uploadStatus(state)),
    ),
  );

  renderApp(`/libraries/${libraryId}/uploads?upload=${uploadId}`);
  expect(await screen.findByText(copy)).toBeInTheDocument();
  expect((await axe.run(document.body)).violations).toEqual([]);
});

async function waitForRequest(): Promise<FakeXMLHttpRequest> {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (FakeXMLHttpRequest.latest !== null) {
      return FakeXMLHttpRequest.latest;
    }
    await act(async () => Promise.resolve());
  }
  throw new Error("upload request was not created");
}
