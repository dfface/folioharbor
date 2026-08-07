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
  deviceId: string;
  pending: PendingPosition[];
  version: number;
}

interface ProgressSyncOptions {
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

const deviceIdKey = "folioharbor.reader.device-id.v1";
const progressKeyPrefix = "folioharbor.reader.progress.v1:";
const maximumPersistedBytes = 64 * 1024;
const maximumPendingPositions = 32;

function progressStorageKey(manifestationId: string, deviceId: string): string {
  return `${progressKeyPrefix}${manifestationId}:${deviceId}`;
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

function readPersisted(storage: Storage | undefined, key: string, deviceId: string): PersistedProgress | null {
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
    return { deviceId, pending: pending as PendingPosition[], version: value.version };
  } catch {
    return null;
  }
}

export function getOrCreateDeviceId(storage: Storage, createId: () => string = () => crypto.randomUUID()): string {
  const existing = storage.getItem(deviceIdKey);
  if (existing !== null && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(existing)) {
    return existing;
  }
  const deviceId = createId();
  storage.setItem(deviceIdKey, deviceId);
  return deviceId;
}

export function resetDeviceId(storage: Storage): void {
  storage.removeItem(deviceIdKey);
  const pendingKeys = Array.from({ length: storage.length }, (_, index) => storage.key(index))
    .filter((key): key is string => key?.startsWith(progressKeyPrefix) === true);
  for (const key of pendingKeys) {
    storage.removeItem(key);
  }
}

export class ProgressSync {
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
  private disposed = false;

  constructor(options: ProgressSyncOptions) {
    this.api = options.api;
    this.clock = options.clock;
    this.debounceMs = options.debounceMs ?? 750;
    this.deviceId = options.deviceId;
    this.manifestationId = options.manifestationId;
    this.mutationId = options.mutationId;
    this.storage = options.storage;
    this.storageKey = progressStorageKey(this.manifestationId, this.deviceId);
    const persisted = readPersisted(this.storage, this.storageKey, this.deviceId);
    if (persisted !== null) {
      this.currentVersion = persisted.version;
      this.pending = persisted.pending;
      if (this.pending.length > 0) {
        this.state = { status: "offline", version: this.currentVersion };
      }
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
    try {
      const global = await this.api.get(this.manifestationId);
      if (this.disposed) {
        return;
      }
      if (global !== null) {
        this.currentVersion = global.version;
        this.currentLocator = global.locator;
      }
      if (this.pending.length === 0) {
        this.transition(global === null
          ? { status: "idle", version: 0 }
          : { status: "synced", version: global.version, locator: global.locator });
      } else {
        this.transition(this.positionState("dirty"));
        await this.flush();
      }
    } catch (error) {
      this.transition(this.positionState(this.errorStatus(error)));
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

  retry(): Promise<void> {
    return this.flush();
  }

  async resolveConflict(choice: ConflictChoice): Promise<void> {
    if (this.conflict === null) {
      return;
    }
    const selected = this.conflict;
    this.conflict = null;
    this.pending = [];
    this.currentVersion = selected.global.version;
    if (choice === "global") {
      this.currentLocator = selected.global.locator ?? undefined;
      this.persist();
      this.transition(this.positionState("synced"));
      return;
    }
    this.currentLocator = selected.device.locator;
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
        baseVersion: next.baseVersion,
        clientMutationId: next.clientMutationId,
        deviceId: this.deviceId,
        locator: next.locator,
      };
      this.activeRequest = request;
      try {
        const result = await this.api.update(this.manifestationId, request, { bounded });
        this.pending.shift();
        if (result.kind === "conflict") {
          this.currentVersion = result.global.version;
          this.currentLocator = result.device.locator;
          this.conflict = { global: result.global, device: result.device };
          this.persist();
          this.transition({
            status: "conflict",
            version: result.global.version,
            locator: result.device.locator,
            global: result.global,
            device: result.device,
          });
          return;
        }
        this.currentVersion = result.progress.version;
        this.currentLocator = result.progress.locator;
        this.persist();
      } catch (error) {
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
