import { useQuery } from "@tanstack/react-query";

import { getUploadStatus } from "./api";

export function uploadStatusQueryKey(libraryId: string, uploadId: string) {
  return ["libraries", libraryId, "uploads", uploadId] as const;
}

const terminalStates = new Set(["ready", "duplicate", "failed", "expired"]);

export function useUploadStatus(libraryId: string, uploadId: string | null, enabled: boolean) {
  return useQuery({
    enabled: enabled && uploadId !== null,
    queryKey: uploadStatusQueryKey(libraryId, uploadId ?? ""),
    queryFn: ({ signal }) => getUploadStatus(libraryId, uploadId ?? "", signal),
    refetchInterval: (query) => terminalStates.has(query.state.data?.state ?? "") ? false : 100,
  });
}
