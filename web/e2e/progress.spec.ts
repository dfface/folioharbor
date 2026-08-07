import { expect, test } from "@playwright/test";

import {
  createCollaborativePair,
  expectStatus,
  generatedEpub,
  login,
  newId,
  responseJson,
  uploadPublication,
} from "./support";

interface ItemDetail {
  manifestation_id: string;
}

test("two simultaneous progress writers produce one durable winner and one explicit conflict", async () => {
  const pair = await createCollaborativePair();
  const deviceB = await login(pair.bob.email, pair.bob.password);
  try {
    const upload = await uploadPublication(
      pair.alice,
      pair.aliceLibrary.library_id,
      generatedEpub("Concurrent Progress"),
    );
    expect(upload.state).toBe("ready");
    if (upload.item_id === null) {
      throw new Error("progress fixture did not produce an Item");
    }
    const itemId = upload.item_id;
    const detailResponse = await pair.bob.api.get(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/items/${itemId}`,
    );
    await expectStatus(detailResponse, 200);
    const detail = await responseJson(detailResponse) as ItemDetail;
    const progressPath = `/api/v1/manifestations/${detail.manifestation_id}/progress`;

    const update = (progression: number, deviceId: string, mutationId: string) => ({
      accountId: pair.bob.userId,
      deviceId,
      clientMutationId: mutationId,
      baseVersion: 0,
      packageId: null,
      contentUnitId: null,
      locator: {
        href: `/api/v1/items/${itemId}/resources/concurrent`,
        type: "application/xhtml+xml",
        locations: { progression, position: 1, totalProgression: progression },
        extensions: { version: 1, values: {} },
      },
    });
    const [first, second] = await Promise.all([
      pair.bob.api.put(progressPath, {
        data: update(0.25, newId(), newId()),
        headers: { "If-Match": '"progress-v0"' },
      }),
      deviceB.api.put(progressPath, {
        data: update(0.75, newId(), newId()),
        headers: { "If-Match": '"progress-v0"' },
      }),
    ]);
    expect([first.status(), second.status()].sort()).toEqual([200, 409]);
    const conflict = first.status() === 409 ? first : second;
    expect(await responseJson(conflict)).toEqual(expect.objectContaining({
      code: "progress_conflict",
      global: expect.objectContaining({ version: 1 }),
      device: expect.objectContaining({ locator: expect.any(Object) }),
    }));

    const observed = await deviceB.api.get(progressPath);
    await expectStatus(observed, 200);
    expect(await responseJson(observed)).toEqual(expect.objectContaining({ version: 1 }));
  } finally {
    await Promise.all([
      pair.alice.api.dispose(),
      pair.bob.api.dispose(),
      deviceB.api.dispose(),
    ]);
  }
});
