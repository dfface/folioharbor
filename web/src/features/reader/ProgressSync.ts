import type { components } from "../../api/generated";
import type { Locator } from "./locator";

type ReadingProgress = components["schemas"]["ReadingProgress"];
type ConflictGlobalReadingProgress = components["schemas"]["ConflictGlobalReadingProgress"];
type DeviceReadingProgress = components["schemas"]["DeviceReadingProgress"];
export type ProgressUpdateRequest = components["schemas"]["UpdateReadingProgressRequest"];

export type ProgressUpdateResult =
  | { kind: "updated"; progress: ReadingProgress }
  | { kind: "conflict"; global: ConflictGlobalReadingProgress; device: DeviceReadingProgress };

export interface ProgressApi {
  get(manifestationId: string): Promise<ReadingProgress | null>;
  update(
    manifestationId: string,
    request: ProgressUpdateRequest,
    options: { bounded: boolean },
  ): Promise<ProgressUpdateResult>;
}

export type ProgressApiErrorKind = "inaccessible" | "offline";

export class ProgressApiError extends Error {
  readonly kind: ProgressApiErrorKind;

  constructor(kind: ProgressApiErrorKind) {
    super(`progress_${kind}`);
    this.name = "ProgressApiError";
    this.kind = kind;
  }
}

export interface ProgressClock {
  now(): number;
  setTimeout(callback: () => void, delayMs: number): unknown;
  clearTimeout(handle: unknown): void;
}

export type ProgressState =
  | { status: "idle"; version: 0; locator?: undefined }
  | { status: "dirty" | "offline" | "saving"; version: number; locator?: Locator }
  | { status: "synced"; version: number; locator?: Locator }
  | {
      status: "conflict";
      version: number;
      locator?: Locator;
      global: ConflictGlobalReadingProgress;
      device: DeviceReadingProgress;
    }
  | { status: "inaccessible"; version: number; locator?: Locator };

interface PendingPosition {
  baseVersion?: number;
  clientMutationId: string;
  createdAt: number;
  locator: Locator;
}

interface PersistedProgress {
  accountId: string;
  deviceId: string;
  pending: PendingPosition[];
  version: number;
}

interface ProgressSyncOptions {
  accountId: string;
  api: ProgressApi;
  clock: ProgressClock;
  debounceMs?: number;
  deviceId: string;
  manifestationId: string;
  mutationId: () => string;
  storage?: Storage;
}

type Listener = (state: ProgressState) => void;
type ConflictChoice = "device" | "global";

const legacyDeviceIdKey = "folioharbor.reader.device-id.v1";
const deviceIdKeyPrefix = "folioharbor.reader.device-id.v2:";
const progressKeyPrefix = "folioharbor.reader.progress.v1:";
const maximumPersistedBytes = 64 * 1024;
const maximumPendingPositions = 32;

function progressStorageKey(accountId: string, manifestationId: string, deviceId: string): string {
  return `${progressKeyPrefix}${accountId}:${manifestationId}:${deviceId}`;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isLocator(value: unknown): value is Locator {
  if (!isObject(value) || typeof value.href !== "string" || !isObject(value.locations)) {
    return false;
  }
  return isObject(value.extensions) && value.extensions.version === 1 && isObject(value.extensions.values);
}

function parsePending(value: unknown): PendingPosition | null {
  if (
    !isObject(value) ||
    typeof value.clientMutationId !== "string" ||
    typeof value.createdAt !== "number" ||
    !isLocator(value.locator) ||
    (value.baseVersion !== undefined && typeof value.baseVersion !== "number")
  ) {
    return null;
  }
  return {
    clientMutationId: value.clientMutationId,
    createdAt: value.createdAt,
    locator: value.locator,
    ...(value.baseVersion === undefined ? {} : { baseVersion: value.baseVersion }),
  };
}

function readPersisted(
  storage: Storage | undefined,
  key: string,
  accountId: string,
  deviceId: string,
): PersistedProgress | null {
  if (storage === undefined) {
    return null;
  }
  try {
    const raw = storage.getItem(key);
    if (raw === null || raw.length > maximumPersistedBytes) {
      return null;
    }
    const value: unknown = JSON.parse(raw);
    if (
      !isObject(value) ||
      value.accountId !== accountId ||
      value.deviceId !== deviceId ||
      typeof value.version !== "number" ||
      !Number.isSafeInteger(value.version) ||
      value.version < 0 ||
      !Array.isArray(value.pending) ||
      value.pending.length > maximumPendingPositions
    ) {
      return null;
    }
    const pending = value.pending.map(parsePending);
    if (pending.some((position) => position === null)) {
      return null;
    }
    return { accountId, deviceId, pending: pending as PendingPosition[], version: value.version };
  } catch {
    return null;
  }
}

function hasPendingProgress(storage: Storage, accountId: string, deviceId: string): boolean {
  const accountPrefix = `${progressKeyPrefix}${accountId}:`;
  const deviceSuffix = `:${deviceId}`;
  return Array.from({ length: storage.length }, (_, index) => storage.key(index))
    .filter((key): key is string => key?.startsWith(accountPrefix) === true && key.endsWith(deviceSuffix))
    .some((key) => (readPersisted(storage, key, accountId, deviceId)?.pending.length ?? 0) > 0);
}

function deviceIdIsClaimedByAnotherAccount(storage: Storage, accountId: string, deviceId: string): boolean {
  const accountKey = `${deviceIdKeyPrefix}${accountId}`;
  return Array.from({ length: storage.length }, (_, index) => storage.key(index))
    .filter((key): key is string => key?.startsWith(deviceIdKeyPrefix) === true && key !== accountKey)
    .some((key) => storage.getItem(key) === deviceId);
}

function migratePendingProgress(
  storage: Storage,
  accountId: string,
  previousDeviceId: string | null,
  deviceId: string,
): void {
  if (previousDeviceId === null || previousDeviceId === deviceId) {
    return;
  }
  const accountPrefix = `${progressKeyPrefix}${accountId}:`;
  const previousSuffix = `:${previousDeviceId}`;
  const pendingKeys = Array.from({ length: storage.length }, (_, index) => storage.key(index))
    .filter((key): key is string => key?.startsWith(accountPrefix) === true && key.endsWith(previousSuffix));
  for (const previousKey of pendingKeys) {
    const raw = storage.getItem(previousKey);
    if (raw === null) {
      continue;
    }
    try {
      const persisted: unknown = JSON.parse(raw);
      if (!isObject(persisted) || persisted.accountId !== accountId || persisted.deviceId !== previousDeviceId) {
        continue;
      }
      const nextKey = `${previousKey.slice(0, -previousSuffix.length)}:${deviceId}`;
      storage.setItem(nextKey, JSON.stringify({ ...persisted, deviceId }));
      storage.removeItem(previousKey);
    } catch {
      // Corrupt records remain ignored and cannot be attributed to another account.
    }
  }
}

export function getOrCreateDeviceId(
  storage: Storage,
  accountId: string,
  createId: () => string = () => crypto.randomUUID(),
): string {
  const accountKey = `${deviceIdKeyPrefix}${accountId}`;
  const existing = storage.getItem(accountKey);
  if (existing !== null && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(existing)) {
    return existing;
  }
  const legacyDeviceId = storage.getItem(legacyDeviceIdKey);
  const canPreserveLegacyIdentity = legacyDeviceId !== null &&
    /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(legacyDeviceId) &&
    hasPendingProgress(storage, accountId, legacyDeviceId) &&
    !deviceIdIsClaimedByAnotherAccount(storage, accountId, legacyDeviceId);
  const deviceId = canPreserveLegacyIdentity ? legacyDeviceId : createId();
  storage.setItem(accountKey, deviceId);
  migratePendingProgress(storage, accountId, legacyDeviceId, deviceId);
  return deviceId;
}

export function resetDeviceId(storage: Storage, accountId: string): void {
  storage.removeItem(`${deviceIdKeyPrefix}${accountId}`);
  const accountPrefix = `${progressKeyPrefix}${accountId}:`;
  const pendingKeys = Array.from({ length: storage.length }, (_, index) => storage.key(index))
    .filter((key): key is string => key?.startsWith(accountPrefix) === true);
  for (const key of pendingKeys) {
    storage.removeItem(key);
  }
}

export class ProgressSync {
  private readonly accountId: string;
  private readonly api: ProgressApi;
  private readonly clock: ProgressClock;
  private readonly debounceMs: number;
  private readonly deviceId: string;
  private readonly manifestationId: string;
  private readonly mutationId: () => string;
  private readonly storage: Storage | undefined;
  private readonly storageKey: string;
  private readonly listeners = new Set<Listener>();
  private currentLocator: Locator | undefined;
  private currentVersion = 0;
  private pending: PendingPosition[] = [];
  private state: ProgressState = { status: "idle", version: 0 };
  private debounceHandle: unknown;
  private activeFlush: Promise<void> | null = null;
  private activeRequest: ProgressUpdateRequest | null = null;
  private boundedRetryMutationId: string | null = null;
  private conflict: { global: ConflictGlobalReadingProgress; device: DeviceReadingProgress } | null = null;
  private initialRead: Promise<void> | null = null;
  private initialReadStatus: "failed" | "loaded" | "loading" | "not_started" = "not_started";
  private authoritativeRevision = 0;
  private disposed = false;

  constructor(options: ProgressSyncOptions) {
    this.accountId = options.accountId;
    this.api = options.api;
    this.clock = options.clock;
    this.debounceMs = options.debounceMs ?? 750;
    this.deviceId = options.deviceId;
    this.manifestationId = options.manifestationId;
    this.mutationId = options.mutationId;
    this.storage = options.storage;
    this.storageKey = progressStorageKey(this.accountId, this.manifestationId, this.deviceId);
    const persisted = readPersisted(this.storage, this.storageKey, this.accountId, this.deviceId);
    if (persisted !== null) {
      this.currentVersion = persisted.version;
      this.pending = persisted.pending;
      if (this.pending.length > 0) {
        this.currentLocator = this.pending.at(-1)?.locator;
        this.state = this.positionState("offline");
      }
    }
    try {
      this.storage?.removeItem(`${progressKeyPrefix}${this.manifestationId}:${this.deviceId}`);
    } catch {
      // Unsafe unscoped records are ignored even when browser storage cannot be cleaned up.
    }
  }

  snapshot(): ProgressState {
    return this.state;
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    listener(this.state);
    return () => this.listeners.delete(listener);
  }

  async start(): Promise<void> {
    await this.loadInitialProgress();
    if (this.initialReadStatus === "loaded" && this.pending.length > 0 && this.conflict === null) {
      await this.flush();
    }
  }

  report(locator: Locator): void {
    if (this.disposed || this.state.status === "inaccessible" || this.state.status === "conflict") {
      return;
    }
    const tail = this.pending.at(-1);
    if (tail !== undefined && tail.baseVersion === undefined) {
      tail.locator = locator;
      tail.createdAt = this.clock.now();
    } else if (this.pending.length < maximumPendingPositions) {
      this.pending.push({
        clientMutationId: this.mutationId(),
        createdAt: this.clock.now(),
        locator,
      });
    } else {
      this.pending[this.pending.length - 1] = {
        clientMutationId: this.mutationId(),
        createdAt: this.clock.now(),
        locator,
      };
    }
    this.currentLocator = locator;
    this.persist();
    this.transition(this.positionState("dirty"));
    this.scheduleFlush();
  }

  async flush(options: { bounded?: boolean } = {}): Promise<void> {
    this.clearDebounce();
    if (this.disposed || this.pending.length === 0 || this.conflict !== null || this.state.status === "inaccessible") {
      return;
    }
    if (this.activeFlush !== null) {
      if (
        options.bounded === true &&
        this.activeRequest !== null &&
        this.boundedRetryMutationId !== this.activeRequest.clientMutationId
      ) {
        const request = this.activeRequest;
        this.boundedRetryMutationId = request.clientMutationId;
        void this.api.update(this.manifestationId, request, { bounded: true }).catch(() => {
          // The original in-flight command remains authoritative; its exact id is persisted for later replay.
        });
      }
      return this.activeFlush;
    }
    const operation = this.drain(options.bounded === true);
    this.activeFlush = operation;
    try {
      await operation;
    } finally {
      if (this.activeFlush === operation) {
        this.activeFlush = null;
      }
    }
  }

  async retry(): Promise<void> {
    if (this.initialReadStatus === "failed") {
      await this.loadInitialProgress();
      if (!this.initialProgressIsLoaded()) {
        return;
      }
    }
    await this.flush();
  }

  async resolveConflict(choice: ConflictChoice): Promise<void> {
    if (this.conflict === null) {
      return;
    }
    const selected = this.conflict;
    this.conflict = null;
    this.currentVersion = selected.global.version;
    this.currentLocator = choice === "global"
      ? selected.global.locator ?? undefined
      : selected.device.locator;
    if (this.pending.length > 0) {
      for (const pending of this.pending) {
        delete pending.baseVersion;
      }
      this.currentLocator = this.pending.at(-1)?.locator ?? this.currentLocator;
      this.persist();
      this.transition(this.positionState("dirty"));
      await this.flush();
      return;
    }
    if (choice === "global") {
      this.persist();
      this.transition(this.positionState("synced"));
      return;
    }
    this.pending.push({
      clientMutationId: this.mutationId(),
      createdAt: this.clock.now(),
      locator: selected.device.locator,
    });
    this.persist();
    this.transition(this.positionState("dirty"));
    await this.flush();
  }

  dispose(): void {
    this.disposed = true;
    this.clearDebounce();
    this.listeners.clear();
  }

  private async drain(bounded: boolean): Promise<void> {
    while (this.pending.length > 0 && !this.disposed) {
      const next = this.pending[0];
      if (next === undefined) {
        return;
      }
      next.baseVersion ??= this.currentVersion;
      this.persist();
      this.transition(this.positionState("saving"));
      const request: ProgressUpdateRequest = {
        accountId: this.accountId,
        baseVersion: next.baseVersion,
        clientMutationId: next.clientMutationId,
        deviceId: this.deviceId,
        locator: next.locator,
      };
      this.activeRequest = request;
      try {
        const result = await this.api.update(this.manifestationId, request, { bounded });
        this.authoritativeRevision += 1;
        this.pending.shift();
        if (result.kind === "conflict") {
          this.currentVersion = result.global.version;
          const latestLocal = this.pending.at(-1)?.locator ?? result.device.locator;
          const device = { ...result.device, locator: latestLocal };
          this.currentLocator = latestLocal;
          this.conflict = { global: result.global, device };
          this.persist();
          this.transition({
            status: "conflict",
            version: result.global.version,
            locator: latestLocal,
            global: result.global,
            device,
          });
          return;
        }
        this.currentVersion = result.progress.version;
        this.currentLocator = result.progress.locator;
        this.persist();
      } catch (error) {
        if (error instanceof ProgressApiError && error.kind === "inaccessible") {
          this.authoritativeRevision += 1;
        }
        this.persist();
        this.transition(this.positionState(this.errorStatus(error)));
        return;
      } finally {
        if (this.activeRequest === request) {
          this.activeRequest = null;
          this.boundedRetryMutationId = null;
        }
      }
    }
    if (!this.disposed) {
      this.transition(this.positionState("synced"));
    }
  }

  private errorStatus(error: unknown): "inaccessible" | "offline" {
    return error instanceof ProgressApiError && error.kind === "inaccessible" ? "inaccessible" : "offline";
  }

  private initialProgressIsLoaded(): boolean {
    return this.initialReadStatus === "loaded";
  }

  private async loadInitialProgress(): Promise<void> {
    if (this.disposed || this.initialReadStatus === "loaded") {
      return;
    }
    if (this.initialRead !== null) {
      return this.initialRead;
    }
    const revisionAtStart = this.authoritativeRevision;
    this.initialReadStatus = "loading";
    const operation = (async () => {
      try {
        const global = await this.api.get(this.manifestationId);
        if (this.disposed) {
          return;
        }
        this.initialReadStatus = "loaded";
        if (
          this.authoritativeRevision !== revisionAtStart ||
          this.state.status === "conflict" ||
          this.state.status === "inaccessible"
        ) {
          return;
        }
        const head = this.pending[0];
        if (this.pending.length > 0 || this.activeRequest !== null) {
          if (this.activeRequest === null && head?.baseVersion === undefined) {
            this.currentVersion = global?.version ?? 0;
          }
          this.transition(this.positionState("dirty"));
          return;
        }
        if (global === null) {
          this.currentVersion = 0;
          this.currentLocator = undefined;
          this.transition({ status: "idle", version: 0 });
        } else {
          this.currentVersion = global.version;
          this.currentLocator = global.locator;
          this.transition({ status: "synced", version: global.version, locator: global.locator });
        }
      } catch (error) {
        if (this.disposed) {
          return;
        }
        if (
          this.authoritativeRevision !== revisionAtStart ||
          this.state.status === "conflict" ||
          this.state.status === "inaccessible"
        ) {
          this.initialReadStatus = "loaded";
          return;
        }
        this.initialReadStatus = "failed";
        this.transition(this.positionState(this.errorStatus(error)));
      }
    })();
    this.initialRead = operation;
    try {
      await operation;
    } finally {
      if (this.initialRead === operation) {
        this.initialRead = null;
      }
    }
  }

  private positionState(status: "dirty" | "inaccessible" | "offline" | "saving" | "synced"): ProgressState {
    return this.currentLocator === undefined
      ? { status, version: this.currentVersion }
      : { status, version: this.currentVersion, locator: this.currentLocator };
  }

  private scheduleFlush(): void {
    this.clearDebounce();
    this.debounceHandle = this.clock.setTimeout(() => {
      this.debounceHandle = undefined;
      void this.flush();
    }, this.debounceMs);
  }

  private clearDebounce(): void {
    if (this.debounceHandle !== undefined) {
      this.clock.clearTimeout(this.debounceHandle);
      this.debounceHandle = undefined;
    }
  }

  private persist(): void {
    if (this.storage === undefined) {
      return;
    }
    try {
      if (this.pending.length === 0) {
        this.storage.removeItem(this.storageKey);
        return;
      }
      const serialized = JSON.stringify({
        accountId: this.accountId,
        deviceId: this.deviceId,
        pending: this.pending,
        version: this.currentVersion,
      } satisfies PersistedProgress);
      if (serialized.length <= maximumPersistedBytes) {
        this.storage.setItem(this.storageKey, serialized);
      } else {
        this.storage.removeItem(this.storageKey);
      }
    } catch {
      // Synchronization remains live when durable browser storage is unavailable.
    }
  }

  private transition(state: ProgressState): void {
    if (this.disposed) {
      return;
    }
    this.state = state;
    for (const listener of this.listeners) {
      listener(state);
    }
  }
}

export const browserProgressClock: ProgressClock = {
  now: () => Date.now(),
  setTimeout: (callback, delayMs) => window.setTimeout(callback, delayMs),
  clearTimeout: (handle) => {
    if (typeof handle === "number") {
      window.clearTimeout(handle);
    }
  },
};
