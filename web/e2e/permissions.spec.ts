import { execFileSync } from "node:child_process";

import { expect, test } from "@playwright/test";

import {
  createCollaborativePair,
  expectStatus,
  generatedEpub,
  login,
  maliciousTraversalEpub,
  readProblemCode,
  registerAndVerify,
  responseJson,
  uniqueEmail,
  uploadPublication,
} from "./support";

interface Manifest {
  readingOrder: { href: string }[];
}

test.describe.configure({ timeout: 120_000 });

test("editors can upload but cannot manage members, while readers cannot upload", async () => {
  const editorPair = await createCollaborativePair("editor");
  const readerPair = await createCollaborativePair("reader");
  try {
    const editorUpload = await editorPair.bob.api.post(
      `/api/v1/libraries/${editorPair.aliceLibrary.library_id}/uploads`,
      {
        data: {
          file_name: "editor.epub",
          media_type: "application/epub+zip",
          declared_bytes: 1,
        },
      },
    );
    await expectStatus(editorUpload, 202);

    const editorInvite = await editorPair.bob.api.post(
      `/api/v1/libraries/${editorPair.aliceLibrary.library_id}/invitations`,
      { data: { email: uniqueEmail("editor-target"), role: "reader" } },
    );
    expect(editorInvite.status()).toBe(403);
    expect(await readProblemCode(editorInvite)).toBe("library_action_forbidden");

    const readerUpload = await readerPair.bob.api.post(
      `/api/v1/libraries/${readerPair.aliceLibrary.library_id}/uploads`,
      {
        data: {
          file_name: "reader.epub",
          media_type: "application/epub+zip",
          declared_bytes: 1,
        },
      },
    );
    expect(readerUpload.status()).toBe(403);
    expect(await readProblemCode(readerUpload)).toBe("library_action_forbidden");
  } finally {
    await Promise.all([
      editorPair.alice.api.dispose(),
      editorPair.bob.api.dispose(),
      readerPair.alice.api.dispose(),
      readerPair.bob.api.dispose(),
    ]);
  }
});

test("wrong-library and unrelated access remains anti-enumerating across every publication route", async () => {
  const pair = await createCollaborativePair();
  const outsiderEmail = uniqueEmail("outsider");
  await registerAndVerify(outsiderEmail);
  const outsider = await login(outsiderEmail);
  try {
    const terminal = await uploadPublication(
      pair.alice,
      pair.aliceLibrary.library_id,
      generatedEpub("Isolation Book"),
    );
    expect(terminal.state).toBe("ready");
    if (terminal.item_id === null) {
      throw new Error("isolation upload did not produce an Item");
    }

    const wrongLibraryRequests = [
      pair.bob.api.get(
        `/api/v1/libraries/${pair.bobPersonalLibrary.library_id}/uploads/${terminal.upload_id}`,
      ),
      pair.bob.api.get(
        `/api/v1/libraries/${pair.bobPersonalLibrary.library_id}/items/${terminal.item_id}`,
      ),
    ];
    const unrelatedRequests = [
      outsider.api.get(`/api/v1/libraries/${pair.aliceLibrary.library_id}`),
      outsider.api.get(`/api/v1/libraries/${pair.aliceLibrary.library_id}/books`),
      outsider.api.get(`/api/v1/items/${terminal.item_id}/manifest`),
      outsider.api.get(`/api/v1/items/${terminal.item_id}/resources/not-a-real-resource`),
      outsider.api.get(`/api/v1/items/${terminal.item_id}/download`),
    ];
    for (const pending of [...wrongLibraryRequests, ...unrelatedRequests]) {
      const response = await pending;
      expect(response.status()).toBe(404);
      const body = await response.text();
      expect(body).not.toContain(terminal.upload_id);
      expect(body).not.toContain(terminal.item_id);
      expect(body).not.toContain(pair.aliceLibrary.library_id);
      expect(body).not.toMatch(/(?:storage_key|sha256|\/var\/lib\/folioharbor|blob:)/iu);
    }
  } finally {
    await Promise.all([pair.alice.api.dispose(), pair.bob.api.dispose(), outsider.api.dispose()]);
  }
});

test("malicious EPUBs fail safely and membership revocation applies on the next resource request", async () => {
  const pair = await createCollaborativePair();
  try {
    const malicious = await uploadPublication(
      pair.alice,
      pair.aliceLibrary.library_id,
      maliciousTraversalEpub(),
      "traversal.epub",
    );
    expect(malicious).toEqual(expect.objectContaining({
      state: "failed",
      error_code: "invalid_epub",
      item_id: null,
    }));

    const ready = await uploadPublication(
      pair.alice,
      pair.aliceLibrary.library_id,
      generatedEpub("Revocation Book"),
    );
    expect(ready.state).toBe("ready");
    if (ready.item_id === null) {
      throw new Error("revocation upload did not produce an Item");
    }
    const manifestResponse = await pair.bob.api.get(`/api/v1/items/${ready.item_id}/manifest`);
    await expectStatus(manifestResponse, 200);
    const manifest = await responseJson(manifestResponse) as Manifest;
    const resourceHref = manifest.readingOrder[0]?.href;
    if (resourceHref === undefined) {
      throw new Error("revocation publication has no readable resource");
    }
    await expectStatus(await pair.bob.api.get(resourceHref), 200);

    const revoked = await pair.alice.api.delete(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/members/${pair.bob.userId}`,
    );
    await expectStatus(revoked, 204);
    const afterRevocation = await pair.bob.api.get(resourceHref);
    expect(afterRevocation.status()).toBe(404);
    expect(await readProblemCode(afterRevocation)).toBe("item_not_found");

    const logs = execFileSync(
      "docker",
      ["compose", "-p", "folioharbor-e2e", "-f", "../tests/e2e/compose.test.yaml", "logs", "--no-color", "api", "worker"],
      { encoding: "utf8" },
    );
    expect(logs).not.toMatch(/(?:folioharbor_session=|folioharbor_csrf=|set-cookie|cookie:|\/var\/lib\/folioharbor|blob:)/iu);
  } finally {
    await Promise.all([pair.alice.api.dispose(), pair.bob.api.dispose()]);
  }
});
