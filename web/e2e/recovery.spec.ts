import { execFileSync } from "node:child_process";
import { request as httpRequest, type ClientRequest } from "node:http";

import { expect, test } from "@playwright/test";

import {
  anonymousApi,
  createCollaborativePair,
  createUpload,
  expectStatus,
  generatedEpub,
  readProblemCode,
  responseJson,
  uploadPublication,
  waitForUpload,
  type SessionClient,
  type UploadView,
} from "./support";

const composePrefix = [
  "compose",
  "-p",
  "folioharbor-e2e",
  "-f",
  "../tests/e2e/compose.test.yaml",
];

test.describe.configure({ timeout: 120_000 });

function compose(...arguments_: string[]): string {
  return execFileSync("docker", [...composePrefix, ...arguments_], { encoding: "utf8" });
}

async function waitForApi(): Promise<void> {
  const api = await anonymousApi();
  try {
    for (let attempt = 0; attempt < 100; attempt += 1) {
      try {
        if ((await api.get("/health/ready")).status() === 200) {
          return;
        }
      } catch {
        // A killed container may reset the connection while the replacement starts.
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  } finally {
    await api.dispose();
  }
  throw new Error("API did not become ready after restart");
}

function restoreRuntime(): void {
  compose("up", "--detach", "--wait", "api", "worker");
}

function beginSlowUpload(
  client: SessionClient,
  libraryId: string,
  uploadId: string,
  declaredBytes: number,
): { request: ClientRequest; outcome: Promise<"error" | "response"> } {
  const url = new URL(
    `/api/v1/libraries/${libraryId}/uploads/${uploadId}/content`,
    process.env.FOLIOHARBOR_E2E_API_URL ?? "http://127.0.0.1:3000",
  );
  let resolveOutcome: ((outcome: "error" | "response") => void) | undefined;
  const outcome = new Promise<"error" | "response">((resolve) => {
    resolveOutcome = resolve;
  });
  const request = httpRequest(url, {
    method: "PUT",
    headers: {
      "Content-Type": "application/epub+zip",
      "Content-Length": String(declaredBytes),
      Cookie: client.cookieHeader,
      "X-CSRF-Token": client.csrfToken,
    },
  });
  request.once("error", () => {
    resolveOutcome?.("error");
  });
  request.once("response", () => {
    resolveOutcome?.("response");
  });
  request.write(Buffer.alloc(1024, 0x41));
  return { request, outcome };
}

test.beforeEach(() => {
  restoreRuntime();
});

async function status(
  client: SessionClient,
  libraryId: string,
  uploadId: string,
): Promise<UploadView> {
  const response = await client.api.get(`/api/v1/libraries/${libraryId}/uploads/${uploadId}`);
  await expectStatus(response, 200);
  return await responseJson(response) as UploadView;
}

function objectCount(): number {
  const output = compose(
    "exec",
    "-T",
    "worker",
    "sh",
    "-c",
    "find /var/lib/folioharbor/objects -type f 2>/dev/null | wc -l",
  );
  return Number.parseInt(output.trim(), 10);
}

function setObjectStoreWritable(writable: boolean): void {
  compose(
    "exec",
    "-u",
    "0",
    "-T",
    "worker",
    "sh",
    "-ec",
    writable
      ? "find /var/lib/folioharbor/objects -type d -exec chmod 0700 {} +; find /var/lib/folioharbor/objects -type f -exec chmod 0600 {} +"
      : "find /var/lib/folioharbor/objects -type d -exec chmod 0500 {} +",
  );
}

function setLibraryQuota(libraryId: string, bytes: number): void {
  if (!validUuid(libraryId)
    || !Number.isSafeInteger(bytes)
    || bytes < 1) {
    throw new Error("invalid quota fixture input");
  }
  const result = compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-c",
    `UPDATE folioharbor.libraries SET quota_limit_bytes=${String(bytes)} WHERE library_id='${libraryId}'::uuid`,
  );
  expect(result).toContain("UPDATE 1");
}

function expireActiveReceipt(uploadId: string): void {
  if (!validUuid(uploadId)) {
    throw new Error("invalid Upload fixture input");
  }
  const receipt = compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-c",
    `UPDATE folioharbor.upload_sessions SET receipt_lease_expires_at=clock_timestamp()-interval '10 minutes' WHERE upload_id='${uploadId}'::uuid AND state='receiving'`,
  );
  expect(receipt).toContain("UPDATE 1");
  const cleanup = compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-c",
    "UPDATE folioharbor.background_jobs SET next_run_at=clock_timestamp() WHERE kind='expire_uploads_and_reservations' AND state='pending'",
  );
  expect(cleanup).toContain("UPDATE 1");
}

function validUuid(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu.test(value);
}

function itemState(itemId: string): string {
  if (!validUuid(itemId)) {
    throw new Error("invalid Item fixture input");
  }
  return compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-Atc",
    `SELECT state FROM folioharbor.items WHERE item_id='${itemId}'::uuid`,
  ).trim();
}

function makeItemPurgeEligible(itemId: string): void {
  if (!validUuid(itemId)) {
    throw new Error("invalid Item fixture input");
  }
  const result = compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-c",
    `WITH boundary AS (SELECT clock_timestamp() AS now) UPDATE folioharbor.items SET deleted_at=boundary.now-interval '7 days',purge_eligible_at=boundary.now FROM boundary WHERE item_id='${itemId}'::uuid AND state='deleted'`,
  );
  expect(result).toContain("UPDATE 1");
}

function makeUnreferencedBlobPurgeEligible(): void {
  const result = compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-c",
    "WITH boundary AS (SELECT clock_timestamp() AS now) UPDATE folioharbor.blob_locations SET purge_pending_at=boundary.now-interval '24 hours',purge_after=boundary.now FROM boundary WHERE state='purge_pending' AND NOT folioharbor.blob_has_authoritative_reference(blob_id)",
  );
  expect(result).toContain("UPDATE 1");
}

function garbageCollectionJob(): string {
  return compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-Atc",
    "SELECT state||'|'||COALESCE(error_code,'') FROM folioharbor.background_jobs WHERE kind='collect_blobs_later'",
  ).trim();
}

function retryGarbageCollectionNow(): void {
  const result = compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-c",
    "UPDATE folioharbor.background_jobs SET next_run_at=clock_timestamp() WHERE kind='collect_blobs_later' AND state='retry_wait' AND error_code='garbage_collection_unavailable'",
  );
  expect(result).toContain("UPDATE 1");
}

function scheduleGarbageCollectionNow(): void {
  const result = compose(
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    "postgres",
    "-d",
    "folioharbor",
    "-c",
    "UPDATE folioharbor.background_jobs SET next_run_at=clock_timestamp() WHERE kind='collect_blobs_later' AND state='pending'",
  );
  expect(result).toContain("UPDATE 1");
}

test("API and Worker process loss converge queued and in-flight imports to accepted terminal states", async () => {
  const pair = await createCollaborativePair();
  try {
    const interrupted = await createUpload(
      pair.alice,
      pair.aliceLibrary.library_id,
      1024 * 1024,
      "interrupted.epub",
    );
    const slowUpload = beginSlowUpload(
      pair.alice,
      pair.aliceLibrary.library_id,
      interrupted.upload_id,
      1024 * 1024,
    );
    let observedReceiving = false;
    for (let attempt = 0; attempt < 100; attempt += 1) {
      if ((await status(pair.alice, pair.aliceLibrary.library_id, interrupted.upload_id)).state === "receiving") {
        observedReceiving = true;
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    expect(observedReceiving, "test must kill API during an active request body").toBe(true);
    compose("kill", "api");
    const interruptedOutcome = await Promise.race([
      slowUpload.outcome,
      new Promise<"timeout">((resolve) => setTimeout(() => {
        resolve("timeout");
      }, 5_000)),
    ]);
    slowUpload.request.destroy();
    expect(interruptedOutcome).toBe("error");
    expireActiveReceipt(interrupted.upload_id);
    compose("start", "api");
    await waitForApi();
    const expiredReceipt = await waitForUpload(
      pair.alice,
      pair.aliceLibrary.library_id,
      interrupted.upload_id,
    );
    expect(expiredReceipt).toEqual(expect.objectContaining({
      state: "failed",
      error_code: "receipt_expired",
    }));

    compose("stop", "worker");
    const promotedBytes = generatedEpub("Promoted Before Worker Restart");
    const promoted = await createUpload(
      pair.alice,
      pair.aliceLibrary.library_id,
      promotedBytes.byteLength,
      "promoted.epub",
    );
    const queued = await pair.alice.api.put(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/uploads/${promoted.upload_id}/content`,
      { data: promotedBytes, headers: { "Content-Type": "application/epub+zip" } },
    );
    await expectStatus(queued, 202);
    expect((await responseJson(queued) as UploadView).state).toBe("queued");

    compose("start", "worker");
    compose("kill", "worker");
    compose("start", "worker");
    const afterBothRestarts = await waitForUpload(
      pair.alice,
      pair.aliceLibrary.library_id,
      promoted.upload_id,
    );
    expect(afterBothRestarts.state).toBe("ready");

    compose("stop", "worker");
    const inFlightBytes = generatedEpub("Catalog Finalization Restart", 8 * 1024 * 1024);
    const inFlight = await createUpload(
      pair.alice,
      pair.aliceLibrary.library_id,
      inFlightBytes.byteLength,
      "catalog-restart.epub",
    );
    await expectStatus(await pair.alice.api.put(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/uploads/${inFlight.upload_id}/content`,
      { data: inFlightBytes, headers: { "Content-Type": "application/epub+zip" } },
    ), 202);
    compose("start", "worker");

    let observedInFlight = false;
    for (let attempt = 0; attempt < 2_000; attempt += 1) {
      const current = await status(pair.alice, pair.aliceLibrary.library_id, inFlight.upload_id);
      if (["validating", "importing"].includes(current.state)) {
        observedInFlight = true;
        compose("kill", "worker");
        break;
      }
      if (["ready", "duplicate", "failed"].includes(current.state)) {
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 2));
    }
    expect(observedInFlight, "test must cut a real in-flight Worker state").toBe(true);
    compose("start", "worker");
    const recovered = await waitForUpload(
      pair.alice,
      pair.aliceLibrary.library_id,
      inFlight.upload_id,
    );
    expect(["ready", "duplicate"]).toContain(recovered.state);
  } finally {
    restoreRuntime();
    await Promise.all([pair.alice.api.dispose(), pair.bob.api.dispose()]);
  }
});

test("quota reservations and same-content imports remain atomic within and across libraries", async () => {
  const quotaPair = await createCollaborativePair();
  const dedupPair = await createCollaborativePair();
  try {
    setLibraryQuota(quotaPair.aliceLibrary.library_id, 100 * 1024 * 1024);
    const quotaRequests = ["quota-a.epub", "quota-b.epub"].map((file_name) =>
      quotaPair.alice.api.post(
        `/api/v1/libraries/${quotaPair.aliceLibrary.library_id}/uploads`,
        {
          data: {
            file_name,
            media_type: "application/epub+zip",
            declared_bytes: 70 * 1024 * 1024,
          },
        },
      ));
    const quotaResponses = await Promise.all(quotaRequests);
    expect(quotaResponses.map((response) => response.status()).sort()).toEqual([202, 409]);
    const rejected = quotaResponses.find((response) => response.status() === 409);
    if (rejected === undefined) {
      throw new Error("simultaneous quota reservation did not produce a rejected request");
    }
    expect(await readProblemCode(rejected)).toBe("library_quota_exceeded");

    const sameLibraryBytes = generatedEpub("Same Library Concurrent");
    const sameLibrary = await Promise.all([
      uploadPublication(dedupPair.alice, dedupPair.aliceLibrary.library_id, sameLibraryBytes, "same-a.epub"),
      uploadPublication(dedupPair.alice, dedupPair.aliceLibrary.library_id, sameLibraryBytes, "same-b.epub"),
    ]);
    expect(sameLibrary.map((upload) => upload.state).sort()).toEqual(["duplicate", "ready"]);
    expect(new Set(sameLibrary.map((upload) => upload.item_id)).size).toBe(1);

    const crossLibraryBytes = generatedEpub("Cross Library Concurrent");
    const crossLibrary = await Promise.all([
      uploadPublication(dedupPair.alice, dedupPair.aliceLibrary.library_id, crossLibraryBytes, "cross-a.epub"),
      uploadPublication(dedupPair.bob, dedupPair.bobPersonalLibrary.library_id, crossLibraryBytes, "cross-b.epub"),
    ]);
    expect(crossLibrary.map((upload) => upload.state).sort()).toEqual(["ready", "ready"]);
    expect(new Set(crossLibrary.map((upload) => upload.item_id)).size).toBe(2);
  } finally {
    await Promise.all([
      quotaPair.alice.api.dispose(),
      quotaPair.bob.api.dispose(),
      dedupPair.alice.api.dispose(),
      dedupPair.bob.api.dispose(),
    ]);
  }
});

test("deleting one shared Item preserves the other and a failed Blob GC is retried", async () => {
  const pair = await createCollaborativePair();
  try {
    const baseline = objectCount();
    const shared = generatedEpub("Shared Blob Lifecycle");
    const [aliceUpload, bobUpload] = await Promise.all([
      uploadPublication(pair.alice, pair.aliceLibrary.library_id, shared, "shared-alice.epub"),
      uploadPublication(pair.bob, pair.bobPersonalLibrary.library_id, shared, "shared-bob.epub"),
    ]);
    expect([aliceUpload.state, bobUpload.state]).toEqual(["ready", "ready"]);
    if (aliceUpload.item_id === null || bobUpload.item_id === null) {
      throw new Error("shared Blob imports did not produce two Items");
    }
    expect(objectCount()).toBe(baseline + 1);

    await expectStatus(await pair.alice.api.delete(
      `/api/v1/libraries/${pair.aliceLibrary.library_id}/items/${aliceUpload.item_id}`,
    ), 204);
    await expectStatus(await pair.bob.api.get(
      `/api/v1/libraries/${pair.bobPersonalLibrary.library_id}/items/${bobUpload.item_id}`,
    ), 200);
    expect(objectCount()).toBe(baseline + 1);

    setObjectStoreWritable(false);
    await expectStatus(await pair.bob.api.delete(
      `/api/v1/libraries/${pair.bobPersonalLibrary.library_id}/items/${bobUpload.item_id}`,
    ), 204);
    makeItemPurgeEligible(aliceUpload.item_id);
    makeItemPurgeEligible(bobUpload.item_id);
    scheduleGarbageCollectionNow();
    for (let attempt = 0; attempt < 100; attempt += 1) {
      if (itemState(bobUpload.item_id) === "purged") {
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    expect(itemState(bobUpload.item_id)).toBe("purged");
    expect(itemState(aliceUpload.item_id)).toBe("purged");
    makeUnreferencedBlobPurgeEligible();
    scheduleGarbageCollectionNow();
    for (let attempt = 0; attempt < 100; attempt += 1) {
      if (garbageCollectionJob() === "retry_wait|garbage_collection_unavailable") {
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    expect(garbageCollectionJob()).toBe("retry_wait|garbage_collection_unavailable");
    expect(objectCount(), "forced storage denial must retain the physical Blob").toBe(baseline + 1);

    setObjectStoreWritable(true);
    retryGarbageCollectionNow();
    for (let attempt = 0; attempt < 100; attempt += 1) {
      if (objectCount() === baseline) {
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    expect(objectCount()).toBe(baseline);
  } finally {
    setObjectStoreWritable(true);
    await Promise.all([pair.alice.api.dispose(), pair.bob.api.dispose()]);
  }
});
