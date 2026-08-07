import { apiClient } from "../../api/client";
import type { components } from "../../api/generated";

export type BookSummary = components["schemas"]["BookSummary"];
export type BookPage = components["schemas"]["BookPage"];
export type ItemDetail = components["schemas"]["ItemDetail"];

function signalOption(signal: AbortSignal | undefined): { signal?: AbortSignal } {
  return signal === undefined ? {} : { signal };
}

export function listBooks(libraryId: string, cursor: string | null, signal?: AbortSignal): Promise<BookPage> {
  const query = new URLSearchParams({ limit: "24" });
  if (cursor !== null) {
    query.set("cursor", cursor);
  }
  return apiClient.request(
    `/api/v1/libraries/${encodeURIComponent(libraryId)}/books?${query.toString()}`,
    signalOption(signal),
  );
}

export function getItem(libraryId: string, itemId: string, signal?: AbortSignal): Promise<ItemDetail> {
  return apiClient.request(
    `/api/v1/libraries/${encodeURIComponent(libraryId)}/items/${encodeURIComponent(itemId)}`,
    signalOption(signal),
  );
}
