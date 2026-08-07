import { apiClient } from "../../api/client";
import type { components } from "../../api/generated";

export type Library = components["schemas"]["Library"];
export type LibraryCapabilities = components["schemas"]["LibraryCapabilities"];
export interface UpdateLibrarySettings {
  name: string;
  reader_download_enabled?: boolean;
}

function signalOption(signal: AbortSignal | undefined): { signal?: AbortSignal } {
  return signal === undefined ? {} : { signal };
}

export function listLibraries(signal?: AbortSignal): Promise<Library[]> {
  return apiClient.request("/api/v1/libraries", signalOption(signal));
}

export function getLibrary(libraryId: string, signal?: AbortSignal): Promise<Library> {
  return apiClient.request(`/api/v1/libraries/${encodeURIComponent(libraryId)}`, signalOption(signal));
}

export function updateLibrarySettings(
  libraryId: string,
  input: UpdateLibrarySettings,
  signal?: AbortSignal,
): Promise<void> {
  return apiClient.request(`/api/v1/libraries/${encodeURIComponent(libraryId)}/settings`, {
    body: input,
    method: "PATCH",
    ...signalOption(signal),
  });
}
