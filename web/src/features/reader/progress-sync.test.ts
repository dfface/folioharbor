import { expect, test, vi } from "vitest";

import type { components } from "../../api/generated";
import { createLocator, type Locator } from "./locator";
import {
  getOrCreateDeviceId,
  ProgressApiError,
  ProgressSync,
  resetDeviceId,
  type ProgressApi,
  type ProgressClock,
  type ProgressUpdateRequest,
} from "./ProgressSync";

type ReadingProgress = components["schemas"]["ReadingProgress"];

const manifestationId = "018f47b5-58b4-7ba6-9a3a-d9f41f17d001";
const accountA = "018f47b5-58b4-7ba6-9a3a-d9f41f17a101";
const accountB = "018f47b5-58b4-7ba6-9a3a-d9f41f17a102";
const deviceA = "018f47b5-58b4-7ba6-9a3a-d9f41f17e001";
const deviceB = "018f47b5-58b4-7ba6-9a3a-d9f41f17e002";

class MemoryStorage implements Storage {
  private readonly values = new Map<string, string>();

  get length() { return this.values.size; }
  clear() { this.values.clear(); }
  getItem(key: string) { return this.values.get(key) ?? null; }
  key(index: number) { return [...this.values.keys()][index] ?? null; }
  removeItem(key: string) { this.values.delete(key); }
  setItem(key: string, value: string) { this.values.set(key, value); }
}

class FakeClock implements ProgressClock {
  private current = 1_700_000_000_000;
  private nextId = 1;
  private readonly timers = new Map<number, { at: number; callback: () => void }>();

  now() { return this.current; }
  setTimeout(callback: () => void, delayMs: number) {
    const id = this.nextId++;
    this.timers.set(id, { at: this.current + delayMs, callback });
    return id;
  }
  clearTimeout(id: unknown) {
    if (typeof id === "number") {
      this.timers.delete(id);
    }
  }
  advanceBy(milliseconds: number) {
    const end = this.current + milliseconds;
    let next = [...this.timers.entries()]
      .filter(([, timer]) => timer.at <= end)
      .sort((left, right) => left[1].at - right[1].at)[0];
    while (next !== undefined) {
      this.current = next[1].at;
      this.timers.delete(next[0]);
      next[1].callback();
      next = [...this.timers.entries()]
        .filter(([, timer]) => timer.at <= end)
        .sort((left, right) => left[1].at - right[1].at)[0];
    }
    this.current = end;
  }
}

function locator(progression: number, chapter = "one"): Locator {
  return createLocator({
    href: `/api/v1/items/book/resources/chapter-${chapter}`,
    mediaType: "application/xhtml+xml",
    position: chapter === "one" ? 1 : 2,
    totalProgression: progression,
  });
}

function progress(version: number, position: Locator): ReadingProgress {
  return {
    manifestationId,
    locator: position,
    version,
    updatedAt: `2026-08-07T00:00:0${String(version)}Z`,
  };
}

function mutationIds(prefix: string) {
  let next = 1;
  return () => `${prefix}-0000-4000-8000-${String(next++).padStart(12, "0")}`;
}

function createSync(
  api: ProgressApi,
  clock: ProgressClock,
  options: { accountId?: string; deviceId?: string; storage?: Storage; mutationPrefix?: string } = {},
) {
  return new ProgressSync({
    accountId: options.accountId ?? accountA,
    api,
    clock,
    debounceMs: 500,
    deviceId: options.deviceId ?? deviceA,
    manifestationId,
    mutationId: mutationIds(options.mutationPrefix ?? "018f47b5"),
    ...(options.storage === undefined ? {} : { storage: options.storage }),
  });
}

test("pending progress is partitioned by authenticated account on one installation", async () => {
  const clock = new FakeClock();
  const storage = new MemoryStorage();
  const accountAApi: ProgressApi = {
    get: () => Promise.resolve(null),
    update: () => Promise.reject(new ProgressApiError("offline")),
  };
  const first = createSync(accountAApi, clock, { accountId: accountA, storage });
  await first.start();
  first.report(locator(0.2));
  await first.flush();
  expect(first.snapshot().status).toBe("offline");

  const accountBRequests: ProgressUpdateRequest[] = [];
  const accountBApi: ProgressApi = {
    get: () => Promise.resolve(null),
    update: (_manifestation, request) => {
      accountBRequests.push(request);
      return Promise.resolve({ kind: "updated", progress: progress(1, request.locator) });
    },
  };
  const second = createSync(accountBApi, clock, { accountId: accountB, storage });
  await second.start();

  expect(accountBRequests).toEqual([]);
  expect(second.snapshot()).toEqual({ status: "idle", version: 0 });
  expect(Array.from({ length: storage.length }, (_, index) => storage.key(index))).toContain(
    `folioharbor.reader.progress.v1:${accountA}:${manifestationId}:${deviceA}`,
  );
});

test("a delayed initial read never regresses a newer accepted write", async () => {
  const clock = new FakeClock();
  let releaseGet: ((saved: ReadingProgress | null) => void) | undefined;
  const delayedGet = new Promise<ReadingProgress | null>((resolve) => { releaseGet = resolve; });
  const requests: ProgressUpdateRequest[] = [];
  const api: ProgressApi = {
    get: () => delayedGet,
    update: (_manifestation, request) => {
      requests.push(structuredClone(request));
      return Promise.resolve({ kind: "updated", progress: progress(request.baseVersion + 1, request.locator) });
    },
  };
  const sync = createSync(api, clock);
  const starting = sync.start();
  sync.report(locator(0.3));
  await sync.flush();
  expect(sync.snapshot()).toMatchObject({ status: "synced", version: 1, locator: locator(0.3) });

  releaseGet?.(null);
  await starting;
  expect(sync.snapshot()).toMatchObject({ status: "synced", version: 1, locator: locator(0.3) });

  sync.report(locator(0.4));
  await sync.flush();
  expect(requests.at(-1)).toMatchObject({ baseVersion: 1, locator: locator(0.4) });
});

test("online retry repeats a failed initial read before there is a local mutation", async () => {
  const clock = new FakeClock();
  let reads = 0;
  const saved = progress(4, locator(0.7, "two"));
  const api: ProgressApi = {
    get: () => {
      reads += 1;
      return reads === 1 ? Promise.reject(new ProgressApiError("offline")) : Promise.resolve(saved);
    },
    update: () => Promise.reject(new Error("unexpected write")),
  };
  const sync = createSync(api, clock);
  await sync.start();
  expect(sync.snapshot()).toMatchObject({ status: "offline", version: 0 });

  await sync.retry();

  expect(reads).toBe(2);
  expect(sync.snapshot()).toMatchObject({ status: "synced", version: 4, locator: saved.locator });
});

test("debounces dirty positions, advances accepted versions, and uses a bounded lifecycle flush", async () => {
  const clock = new FakeClock();
  const requests: { request: ProgressUpdateRequest; bounded: boolean }[] = [];
  const api: ProgressApi = {
    get: () => Promise.resolve(null),
    update: (_manifestation, request, options) => {
      requests.push({ request, bounded: options.bounded });
      return Promise.resolve({ kind: "updated", progress: progress(request.baseVersion + 1, request.locator) });
    },
  };
  const sync = createSync(api, clock);
  await sync.start();

  sync.report(locator(0.2));
  expect(sync.snapshot().status).toBe("dirty");
  clock.advanceBy(499);
  expect(requests).toEqual([]);
  clock.advanceBy(1);
  await vi.waitFor(() => { expect(sync.snapshot()).toMatchObject({ status: "synced", version: 1 }); });
  expect(requests[0]).toMatchObject({ bounded: false, request: { baseVersion: 0, locator: locator(0.2) } });

  sync.report(locator(0.3));
  await sync.flush({ bounded: true });
  expect(requests[1]).toMatchObject({ bounded: true, request: { baseVersion: 1, locator: locator(0.3) } });
  expect(sync.snapshot()).toMatchObject({ status: "synced", version: 2, locator: locator(0.3) });
});

test("an offline retry reuses the exact mutation command and clears persistence only after acceptance", async () => {
  const clock = new FakeClock();
  const storage = new MemoryStorage();
  const requests: ProgressUpdateRequest[] = [];
  let online = false;
  const api: ProgressApi = {
    get: () => Promise.resolve(null),
    update: (_manifestation, request) => {
      requests.push(structuredClone(request));
      if (!online) {
        return Promise.reject(new ProgressApiError("offline"));
      }
      return Promise.resolve({ kind: "updated", progress: progress(1, request.locator) });
    },
  };
  const sync = createSync(api, clock, { storage });
  await sync.start();
  sync.report(locator(0.25));
  clock.advanceBy(500);
  await vi.waitFor(() => { expect(sync.snapshot().status).toBe("offline"); });

  const persisted = storage.getItem(`folioharbor.reader.progress.v1:${accountA}:${manifestationId}:${deviceA}`);
  expect(persisted).toContain(requests[0]?.clientMutationId);
  online = true;
  await sync.retry();

  expect(requests).toHaveLength(2);
  expect(requests[1]).toEqual(requests[0]);
  expect(sync.snapshot()).toMatchObject({ status: "synced", version: 1 });
  expect(storage.length).toBe(0);
});

test.each([
  ["owner first", [accountA, accountB]],
  ["other account first", [accountB, accountA]],
] as const)("an unambiguous legacy pending command replays exactly with %s", async (_label, accessOrder) => {
  const clock = new FakeClock();
  const storage = new MemoryStorage();
  const accountAReplacement = "018f47b5-58b4-7ba6-9a3a-d9f41f17e003";
  const committedRequest: ProgressUpdateRequest = {
    accountId: accountA,
    baseVersion: 0,
    clientMutationId: "018f47b5-0000-4000-8000-000000000099",
    deviceId: deviceA,
    locator: locator(0.3),
  };
  const committedProgress = progress(1, committedRequest.locator);
  storage.setItem("folioharbor.reader.device-id.v1", deviceA);
  storage.setItem(
    `folioharbor.reader.progress.v1:${accountA}:${manifestationId}:${deviceA}`,
    JSON.stringify({
      accountId: accountA,
      deviceId: deviceA,
      pending: [{
        baseVersion: committedRequest.baseVersion,
        clientMutationId: committedRequest.clientMutationId,
        createdAt: 1_700_000_000_000,
        locator: committedRequest.locator,
      }],
      version: 0,
    }),
  );
  const requests: ProgressUpdateRequest[] = [];
  const api: ProgressApi = {
    get: () => Promise.resolve(structuredClone(committedProgress)),
    update: (_manifestation, request) => {
      requests.push(structuredClone(request));
      if (JSON.stringify(request) !== JSON.stringify(committedRequest)) {
        return Promise.reject(new ProgressApiError("offline"));
      }
      return Promise.resolve({ kind: "updated", progress: structuredClone(committedProgress) });
    },
  };

  const generatedDevices = new Map([
    [accountA, accountAReplacement],
    [accountB, deviceB],
  ]);
  const devices = new Map<string, string>();
  for (const accountId of accessOrder) {
    devices.set(
      accountId,
      getOrCreateDeviceId(storage, accountId, () => generatedDevices.get(accountId) ?? "unexpected"),
    );
  }
  const accountADevice = devices.get(accountA) ?? "missing";
  const sync = createSync(api, clock, { accountId: accountA, deviceId: accountADevice, storage });
  await sync.start();

  expect(devices.get(accountB)).toBe(deviceB);
  expect(accountADevice).toBe(deviceA);
  expect(requests).toEqual([committedRequest]);
  expect(sync.snapshot()).toMatchObject({ status: "synced", version: 1, locator: committedRequest.locator });
  expect(storage.getItem(
    `folioharbor.reader.progress.v1:${accountA}:${manifestationId}:${deviceA}`,
  )).toBeNull();
});

test.each([
  ["account A then account B", [accountA, accountB]],
  ["account B then account A", [accountB, accountA]],
] as const)("ambiguous legacy queues remain exact and unclaimed for %s", (_label, accessOrder) => {
  const storage = new MemoryStorage();
  const accountAReplacement = "018f47b5-58b4-7ba6-9a3a-d9f41f17e003";
  const legacyQueues = new Map([
    [accountA, JSON.stringify({
      accountId: accountA,
      deviceId: deviceA,
      pending: [{
        baseVersion: 4,
        clientMutationId: "018f47b5-0000-4000-8000-0000000000a1",
        createdAt: 1_700_000_000_000,
        locator: locator(0.3),
      }],
      version: 4,
    })],
    [accountB, JSON.stringify({
      accountId: accountB,
      deviceId: deviceA,
      pending: [{
        baseVersion: 7,
        clientMutationId: "018f47b5-0000-4000-8000-0000000000b1",
        createdAt: 1_700_000_000_001,
        locator: locator(0.6, "two"),
      }],
      version: 7,
    })],
  ]);
  storage.setItem("folioharbor.reader.device-id.v1", deviceA);
  for (const [accountId, queue] of legacyQueues) {
    storage.setItem(
      `folioharbor.reader.progress.v1:${accountId}:${manifestationId}:${deviceA}`,
      queue,
    );
  }
  const generatedDevices = new Map([
    [accountA, accountAReplacement],
    [accountB, deviceB],
  ]);
  const devices = new Map<string, string>();

  for (const accountId of accessOrder) {
    devices.set(
      accountId,
      getOrCreateDeviceId(storage, accountId, () => generatedDevices.get(accountId) ?? "unexpected"),
    );
  }

  expect(devices).toEqual(new Map([
    [accessOrder[0], generatedDevices.get(accessOrder[0])],
    [accessOrder[1], generatedDevices.get(accessOrder[1])],
  ]));
  for (const [accountId, queue] of legacyQueues) {
    const generatedDevice = generatedDevices.get(accountId) ?? "missing";
    expect(storage.getItem(
      `folioharbor.reader.progress.v1:${accountId}:${manifestationId}:${deviceA}`,
    )).toBe(queue);
    expect(storage.getItem(
      `folioharbor.reader.progress.v1:${accountId}:${manifestationId}:${generatedDevice}`,
    )).toBeNull();
  }
});

test("a bounded lifecycle flush duplicates an in-flight command with the same mutation id for safe delivery", async () => {
  const clock = new FakeClock();
  const calls: { bounded: boolean; request: ProgressUpdateRequest }[] = [];
  let releaseFirst: ((result: Awaited<ReturnType<ProgressApi["update"]>>) => void) | undefined;
  const firstPending = new Promise<Awaited<ReturnType<ProgressApi["update"]>>>((resolve) => {
    releaseFirst = resolve;
  });
  const api: ProgressApi = {
    get: () => Promise.resolve(null),
    update: (_manifestation, request, options) => {
      calls.push({ bounded: options.bounded, request: structuredClone(request) });
      return calls.length === 1
        ? firstPending
        : Promise.resolve({ kind: "updated", progress: progress(1, request.locator) });
    },
  };
  const sync = createSync(api, clock);
  await sync.start();
  sync.report(locator(0.35));
  const ordinaryFlush = sync.flush();
  await vi.waitFor(() => { expect(sync.snapshot().status).toBe("saving"); });

  const lifecycleFlush = sync.flush({ bounded: true });
  await vi.waitFor(() => { expect(calls).toHaveLength(2); });
  expect(calls[1]).toEqual({ bounded: true, request: calls[0]?.request });

  releaseFirst?.({ kind: "updated", progress: progress(1, locator(0.35)) });
  await Promise.all([ordinaryFlush, lifecycleFlush]);
  expect(sync.snapshot()).toMatchObject({ status: "synced", version: 1 });
});

class SharedProgressApi implements ProgressApi {
  global: ReadingProgress | null = null;
  readonly requests: ProgressUpdateRequest[] = [];

  get() {
    return Promise.resolve(this.global === null ? null : structuredClone(this.global));
  }

  update(_manifestation: string, request: ProgressUpdateRequest): ReturnType<ProgressApi["update"]> {
    this.requests.push(structuredClone(request));
    const version = this.global?.version ?? 0;
    if (request.baseVersion !== version) {
      return Promise.resolve({
        kind: "conflict" as const,
        global: this.global === null
          ? { manifestationId, locator: null, version: 0, updatedAt: null }
          : { ...structuredClone(this.global), locator: structuredClone(this.global.locator) },
        device: {
          deviceId: request.deviceId,
          locator: structuredClone(request.locator),
          updatedAt: "2026-08-07T00:00:09Z",
        },
      });
    }
    this.global = progress(version + 1, structuredClone(request.locator));
    return Promise.resolve({ kind: "updated" as const, progress: structuredClone(this.global) });
  }
}

test("two devices expose stale global/device choices and choosing the smaller device position never applies max-percentage", async () => {
  const api = new SharedProgressApi();
  const clock = new FakeClock();
  const first = createSync(api, clock, { deviceId: deviceA, mutationPrefix: "aaaaaaaa" });
  const second = createSync(api, clock, { deviceId: deviceB, mutationPrefix: "bbbbbbbb" });
  await Promise.all([first.start(), second.start()]);

  first.report(locator(0.9));
  await first.flush();
  second.report(locator(0.1));
  await second.flush();

  expect(second.snapshot()).toMatchObject({
    status: "conflict",
    global: { version: 1, locator: locator(0.9) },
    device: { deviceId: deviceB, locator: locator(0.1) },
  });
  await second.resolveConflict("device");

  expect(api.global).toMatchObject({ version: 2, locator: locator(0.1) });
  expect(api.requests.at(-1)).toMatchObject({ baseVersion: 1, deviceId: deviceB, locator: locator(0.1) });
  expect(second.snapshot()).toMatchObject({ status: "synced", version: 2, locator: locator(0.1) });
});

test.each(["device", "global"] as const)(
  "choosing %s after an in-flight conflict rebases the newer queued device position instead of dropping it",
  async (choice) => {
    const clock = new FakeClock();
    const requests: ProgressUpdateRequest[] = [];
    let releaseConflict: ((result: Awaited<ReturnType<ProgressApi["update"]>>) => void) | undefined;
    const conflictResponse = new Promise<Awaited<ReturnType<ProgressApi["update"]>>>((resolve) => {
      releaseConflict = resolve;
    });
    const api: ProgressApi = {
      get: () => Promise.resolve(progress(1, locator(0.8))),
      update: (_manifestation, request) => {
        requests.push(structuredClone(request));
        if (requests.length === 1) {
          return conflictResponse;
        }
        return Promise.resolve({ kind: "updated", progress: progress(3, request.locator) });
      },
    };
    const sync = createSync(api, clock);
    await sync.start();
    sync.report(locator(0.2));
    const firstFlush = sync.flush();
    await vi.waitFor(() => { expect(requests).toHaveLength(1); });
    sync.report(locator(0.6, "two"));
    releaseConflict?.({
      kind: "conflict",
      global: {
        manifestationId,
        locator: locator(0.8),
        version: 2,
        updatedAt: "2026-08-07T00:00:08Z",
      },
      device: {
        deviceId: deviceA,
        locator: locator(0.2),
        updatedAt: "2026-08-07T00:00:09Z",
      },
    });
    await firstFlush;

    expect(sync.snapshot()).toMatchObject({
      status: "conflict",
      device: { locator: locator(0.6, "two") },
      locator: locator(0.6, "two"),
    });
    await sync.resolveConflict(choice);

    expect(requests).toHaveLength(2);
    expect(requests[1]).toMatchObject({ baseVersion: 2, locator: locator(0.6, "two") });
    expect(sync.snapshot()).toMatchObject({ status: "synced", version: 3, locator: locator(0.6, "two") });
  },
);

test("choosing the global conflict position performs no write and updates the subscriber snapshot", async () => {
  const api = new SharedProgressApi();
  const clock = new FakeClock();
  const first = createSync(api, clock, { deviceId: deviceA });
  const second = createSync(api, clock, { deviceId: deviceB, mutationPrefix: "cccccccc" });
  const observed: string[] = [];
  second.subscribe((state) => observed.push(state.status));
  await Promise.all([first.start(), second.start()]);
  first.report(locator(0.4));
  await first.flush();
  second.report(locator(0.8));
  await second.flush();
  const writesBeforeChoice = api.requests.length;

  await second.resolveConflict("global");

  expect(api.requests).toHaveLength(writesBeforeChoice);
  expect(second.snapshot()).toMatchObject({ status: "synced", version: 1, locator: locator(0.4) });
  expect(observed).toContain("conflict");
  expect(observed.at(-1)).toBe("synced");
});

test("offline positions replay in report order with each later base derived from the accepted version", async () => {
  const clock = new FakeClock();
  let online = false;
  const requests: ProgressUpdateRequest[] = [];
  const api: ProgressApi = {
    get: () => Promise.resolve(null),
    update: (_manifestation, request) => {
      requests.push(structuredClone(request));
      if (!online) {
        return Promise.reject(new ProgressApiError("offline"));
      }
      return Promise.resolve({ kind: "updated", progress: progress(request.baseVersion + 1, request.locator) });
    },
  };
  const sync = createSync(api, clock);
  await sync.start();
  sync.report(locator(0.2));
  clock.advanceBy(500);
  await vi.waitFor(() => { expect(sync.snapshot().status).toBe("offline"); });
  sync.report(locator(0.4, "two"));

  online = true;
  await sync.retry();

  const successful = requests.slice(1);
  expect(successful.map(({ locator: position }) => position)).toEqual([locator(0.2), locator(0.4, "two")]);
  expect(successful.map(({ baseVersion }) => baseVersion)).toEqual([0, 1]);
  expect(sync.snapshot()).toMatchObject({ status: "synced", version: 2, locator: locator(0.4, "two") });
});

test("permission loss moves pending progress to inaccessible without discarding its safe retry record", async () => {
  const clock = new FakeClock();
  const storage = new MemoryStorage();
  const api: ProgressApi = {
    get: () => Promise.resolve(null),
    update: () => Promise.reject(new ProgressApiError("inaccessible")),
  };
  const sync = createSync(api, clock, { storage });
  await sync.start();
  sync.report(locator(0.6));
  await sync.flush();

  expect(sync.snapshot().status).toBe("inaccessible");
  expect(storage.length).toBe(1);
});

test("device identity is stable per account on one install and reset preserves every other account", () => {
  const storage = new MemoryStorage();
  const accountAReplacement = "018f47b5-58b4-7ba6-9a3a-d9f41f17e003";
  const generated = [deviceA, deviceB, accountAReplacement];
  const createId = () => generated.shift() ?? "unexpected";

  expect(getOrCreateDeviceId(storage, accountA, createId)).toBe(deviceA);
  expect(getOrCreateDeviceId(storage, accountA, createId)).toBe(deviceA);
  expect(getOrCreateDeviceId(storage, accountB, createId)).toBe(deviceB);
  storage.setItem(
    `folioharbor.reader.progress.v1:${accountA}:${manifestationId}:${deviceA}`,
    "account-a-pending",
  );
  storage.setItem(
    `folioharbor.reader.progress.v1:${accountB}:${manifestationId}:${deviceB}`,
    "account-b-pending",
  );

  resetDeviceId(storage, accountA);

  expect(getOrCreateDeviceId(storage, accountA, createId)).toBe(accountAReplacement);
  expect(getOrCreateDeviceId(storage, accountB, createId)).toBe(deviceB);
  expect(storage.getItem(
    `folioharbor.reader.progress.v1:${accountA}:${manifestationId}:${deviceA}`,
  )).toBeNull();
  expect(storage.getItem(
    `folioharbor.reader.progress.v1:${accountB}:${manifestationId}:${deviceB}`,
  )).toBe("account-b-pending");
});
