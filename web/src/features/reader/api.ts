import { apiClient } from "../../api/client";
import type { components } from "../../api/generated";
import { ApiProblem } from "../../api/problem";
import {
  ProgressApiError,
  type ProgressApi,
  type ProgressUpdateRequest,
  type ProgressUpdateResult,
} from "./ProgressSync";

export type PublicationManifest = components["schemas"]["PublicationManifest"];
export type PublicationLink = components["schemas"]["PublicationLink"];
type ReadingProgress = components["schemas"]["ReadingProgress"];
type ProgressConflictProblem = components["schemas"]["ProgressConflictProblem"];

const resourceTypes = new Set([
  "application/xhtml+xml",
  "text/html",
  "text/css",
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "font/woff",
  "font/woff2",
]);

export class UnsafeReaderResourceError extends Error {
  constructor() {
    super("unsafe_reader_resource");
    this.name = "UnsafeReaderResourceError";
  }
}

export class ReaderResourceError extends Error {
  readonly status: number;

  constructor(status: number) {
    super(`reader_resource_${String(status)}`);
    this.name = "ReaderResourceError";
    this.status = status;
  }
}

function appOrigin(): string {
  return typeof window === "undefined" ? "http://localhost" : window.location.origin;
}

function expectedResourcePrefix(itemId: string): string {
  return `/api/v1/items/${encodeURIComponent(itemId)}/resources/`;
}

export function authorizedResourceHref(itemId: string, href: string): string {
  const parsed = new URL(href, appOrigin());
  const prefix = expectedResourcePrefix(itemId);
  const opaqueId = parsed.pathname.slice(prefix.length);
  if (
    parsed.origin !== appOrigin() ||
    !parsed.pathname.startsWith(prefix) ||
    parsed.search !== "" ||
    !/^[A-Za-z0-9_-]{1,128}$/.test(opaqueId)
  ) {
    throw new UnsafeReaderResourceError();
  }
  return `${parsed.pathname}${parsed.hash}`;
}

function validateManifest(itemId: string, manifest: PublicationManifest): PublicationManifest {
  if (manifest.readingOrder.length === 0) {
    throw new UnsafeReaderResourceError();
  }
  for (const link of [...manifest.readingOrder, ...manifest.resources, ...manifest.toc]) {
    authorizedResourceHref(itemId, link.href);
  }
  const expectedManifestPath = `/api/v1/items/${encodeURIComponent(itemId)}/manifest`;
  for (const link of manifest.links) {
    const parsed = new URL(link.href, appOrigin());
    if (parsed.origin !== appOrigin() || parsed.pathname !== expectedManifestPath || parsed.search !== "") {
      throw new UnsafeReaderResourceError();
    }
  }
  return manifest;
}

export async function getPublicationManifest(itemId: string, signal?: AbortSignal): Promise<PublicationManifest> {
  const manifest = await apiClient.request<PublicationManifest>(
    `/api/v1/items/${encodeURIComponent(itemId)}/manifest`,
    signal === undefined ? {} : { signal },
  );
  return validateManifest(itemId, manifest);
}

export async function getPublicationResource(
  itemId: string,
  link: PublicationLink,
  signal?: AbortSignal,
): Promise<Blob> {
  const href = authorizedResourceHref(itemId, link.href);
  const response = await fetch(new URL(href.split("#", 1)[0] ?? href, appOrigin()), {
    credentials: "include",
    ...(signal === undefined ? {} : { signal }),
  });
  if (!response.ok) {
    throw new ReaderResourceError(response.status);
  }
  const contentType = response.headers.get("Content-Type")?.split(";", 1)[0]?.trim().toLowerCase();
  if (contentType === undefined || !resourceTypes.has(contentType)) {
    throw new UnsafeReaderResourceError();
  }
  return response.blob();
}

function isLocator(value: unknown): value is components["schemas"]["Locator"] {
  return typeof value === "object" && value !== null && "href" in value && typeof value.href === "string" &&
    "locations" in value && typeof value.locations === "object" && value.locations !== null &&
    "extensions" in value && typeof value.extensions === "object" && value.extensions !== null;
}

function isProgressConflict(problem: unknown): problem is ProgressConflictProblem {
  if (typeof problem !== "object" || problem === null || !("code" in problem) || problem.code !== "progress_conflict") {
    return false;
  }
  if (!("global" in problem) || typeof problem.global !== "object" || problem.global === null) {
    return false;
  }
  if (!("device" in problem) || typeof problem.device !== "object" || problem.device === null) {
    return false;
  }
  const global = problem.global as Record<string, unknown>;
  const device = problem.device as Record<string, unknown>;
  return typeof global.manifestationId === "string" && typeof global.version === "number" &&
    (global.locator === null || isLocator(global.locator)) &&
    typeof device.deviceId === "string" && isLocator(device.locator) && typeof device.updatedAt === "string";
}

function progressFailure(error: unknown): never {
  if (error instanceof ApiProblem && [401, 403, 404].includes(error.problem.status)) {
    throw new ProgressApiError("inaccessible");
  }
  throw new ProgressApiError("offline");
}

async function getReadingProgress(manifestationId: string): Promise<ReadingProgress | null> {
  try {
    const result = await apiClient.request<ReadingProgress | undefined>(
      `/api/v1/manifestations/${encodeURIComponent(manifestationId)}/progress`,
    );
    return result ?? null;
  } catch (error) {
    progressFailure(error);
  }
}

async function updateReadingProgress(
  manifestationId: string,
  request: ProgressUpdateRequest,
  options: { bounded: boolean },
): Promise<ProgressUpdateResult> {
  try {
    const progress = await apiClient.request<ReadingProgress>(
      `/api/v1/manifestations/${encodeURIComponent(manifestationId)}/progress`,
      {
        body: request,
        headers: { "If-Match": `"progress-v${String(request.baseVersion)}"` },
        keepalive: options.bounded,
        method: "PUT",
      },
    );
    return { kind: "updated", progress };
  } catch (error) {
    if (error instanceof ApiProblem && isProgressConflict(error.problem)) {
      return { kind: "conflict", global: error.problem.global, device: error.problem.device };
    }
    progressFailure(error);
  }
}

export const readerProgressApi: ProgressApi = {
  get: getReadingProgress,
  update: updateReadingProgress,
};
