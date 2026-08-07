import { expect, test } from "@playwright/test";

import { capturedServiceLogs, SensitiveCaptureGate, storageKeyForUpload } from "./security-scan";
import {
  createCollaborativePair,
  expectStatus,
  generatedEpub,
  infrastructureSecrets,
  login,
  maliciousTraversalEpub,
  readProblemCode,
  registerAndVerify,
  responseJson,
  sha256,
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
    const editorUpload = await uploadPublication(
      editorPair.bob,
      editorPair.aliceLibrary.library_id,
      generatedEpub("Editor Upload"),
      "editor.epub",
    );
    expect(editorUpload.state).toBe("ready");
    expect(editorUpload.item_id).not.toBeNull();
    const editorCatalog = await editorPair.bob.api.get(
      `/api/v1/libraries/${editorPair.aliceLibrary.library_id}/books`,
    );
    await expectStatus(editorCatalog, 200);
    expect(JSON.stringify(await responseJson(editorCatalog))).toContain("Editor Upload");

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
  const outsiderRegistration = await registerAndVerify(outsiderEmail);
  const outsider = await login(outsiderEmail, outsiderRegistration.password);
  try {
    const epub = generatedEpub("Isolation Book");
    const terminal = await uploadPublication(
      pair.alice,
      pair.aliceLibrary.library_id,
      epub,
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
    const gate = new SensitiveCaptureGate([
      ...pair.sensitiveValues,
      ...outsider.sensitiveValues,
      outsiderRegistration.verificationToken,
      ...infrastructureSecrets(),
      sha256(epub),
      storageKeyForUpload(terminal.upload_id),
      "/var/lib/folioharbor",
    ]);
    for (const [index, pending] of [...wrongLibraryRequests, ...unrelatedRequests].entries()) {
      const response = await pending;
      expect(response.status()).toBe(404);
      gate.addSentinels([
        terminal.upload_id,
        terminal.item_id,
        pair.aliceLibrary.library_id,
      ]);
      await gate.captureResponse(`anti-enumeration response ${String(index)}`, response);
    }
    gate.capture("API and Worker logs", capturedServiceLogs());
    gate.assertSafe();
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

    const epub = generatedEpub("Revocation Book");
    const ready = await uploadPublication(
      pair.alice,
      pair.aliceLibrary.library_id,
      epub,
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
    const beforeRevocation = await pair.bob.api.get(resourceHref);
    await expectStatus(beforeRevocation, 200);

    const revoked = await pair.alice.api.delete(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/members/${pair.bob.userId}`,
    );
    await expectStatus(revoked, 204);
    const afterRevocation = await pair.bob.api.get(resourceHref);
    expect(afterRevocation.status()).toBe(404);
    expect(await readProblemCode(afterRevocation)).toBe("item_not_found");

    const gate = new SensitiveCaptureGate([
      ...pair.sensitiveValues,
      ...infrastructureSecrets(),
      sha256(epub),
      storageKeyForUpload(ready.upload_id),
      "/var/lib/folioharbor",
    ]);
    await gate.captureResponse("successful manifest", manifestResponse);
    await gate.captureResponse("successful resource", beforeRevocation);
    await gate.captureResponse("revoked resource", afterRevocation);
    gate.capture("API and Worker logs", capturedServiceLogs());
    gate.assertSafe();
  } finally {
    await Promise.all([pair.alice.api.dispose(), pair.bob.api.dispose()]);
  }
});
