import { expect, test } from "@playwright/test";

import {
  createCollaborativePair,
  expectStatus,
  generatedEpub,
  newId,
  responseJson,
  sha256,
  uploadPublication,
} from "./support";

interface BookPage {
  items: { item_id: string; can_download: boolean; can_read: boolean }[];
}

interface ItemDetail {
  item_id: string;
  manifestation_id: string;
}

interface Manifest {
  manifestationId: string;
  metadata: { title: string };
  readingOrder: { href: string; type: string }[];
}

test("Alice uploads, Bob reads and syncs progress, then a permissioned Range download preserves bytes", async () => {
  const pair = await createCollaborativePair();
  try {
    const epub = generatedEpub();
    const terminal = await uploadPublication(
      pair.alice,
      pair.aliceLibrary.library_id,
      epub,
    );
    expect(terminal.state, terminal.error_code ?? "no worker error").toBe("ready");
    expect(terminal.item_id).not.toBeNull();
    const itemId = terminal.item_id;
    if (itemId === null) {
      throw new Error("ready upload did not expose an Item");
    }

    const catalogResponse = await pair.bob.api.get(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/books`,
    );
    await expectStatus(catalogResponse, 200);
    const catalog = await responseJson(catalogResponse) as BookPage;
    expect(catalog.items).toEqual([
      expect.objectContaining({ item_id: itemId, can_read: true, can_download: false }),
    ]);

    const detailResponse = await pair.bob.api.get(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/items/${itemId}`,
    );
    await expectStatus(detailResponse, 200);
    const detail = await responseJson(detailResponse) as ItemDetail;
    const manifestResponse = await pair.bob.api.get(`/api/v1/items/${itemId}/manifest`);
    await expectStatus(manifestResponse, 200);
    const manifest = await responseJson(manifestResponse) as Manifest;
    expect(manifest.metadata.title).toBe("Generated E2E Book");
    expect(manifest.manifestationId).toBe(detail.manifestation_id);
    const chapter = manifest.readingOrder[0];
    if (chapter === undefined) {
      throw new Error("publication manifest did not expose reading order");
    }
    const resource = await pair.bob.api.get(chapter.href);
    await expectStatus(resource, 200);
    expect(await resource.text()).toContain("complete vertical slice is readable");

    const locator = {
      href: chapter.href,
      type: chapter.type,
      locations: { progression: 0.5, position: 1, totalProgression: 0.5 },
      extensions: { version: 1, values: {} },
    };
    const saved = await pair.bob.api.put(
      `/api/v1/manifestations/${detail.manifestation_id}/progress`,
      {
        data: {
          accountId: pair.bob.userId,
          deviceId: newId(),
          clientMutationId: newId(),
          baseVersion: 0,
          packageId: null,
          contentUnitId: null,
          locator,
        },
        headers: { "If-Match": '"progress-v0"' },
      },
    );
    await expectStatus(saved, 200);
    expect(await responseJson(saved)).toEqual(expect.objectContaining({ version: 1, locator }));

    const observedOnDeviceB = await pair.bob.api.get(
      `/api/v1/manifestations/${detail.manifestation_id}/progress`,
    );
    await expectStatus(observedOnDeviceB, 200);
    expect(await responseJson(observedOnDeviceB)).toEqual(
      expect.objectContaining({ version: 1, locator }),
    );

    const denied = await pair.bob.api.get(`/api/v1/items/${itemId}/download`);
    expect(denied.status()).toBe(403);

    const enabled = await pair.alice.api.patch(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/settings`,
      {
        data: {
          name: pair.aliceLibrary.name,
          reader_download_enabled: true,
        },
      },
    );
    await expectStatus(enabled, 204);

    const downloaded = await pair.bob.api.get(`/api/v1/items/${itemId}/download`, {
      headers: { Range: `bytes=0-${String(epub.byteLength - 1)}` },
    });
    await expectStatus(downloaded, 206);
    expect(downloaded.headers()["content-range"]).toBe(`bytes 0-${String(epub.byteLength - 1)}/${String(epub.byteLength)}`);
    expect(sha256(await downloaded.body())).toBe(sha256(epub));
  } finally {
    await Promise.all([pair.alice.api.dispose(), pair.bob.api.dispose()]);
  }
});
