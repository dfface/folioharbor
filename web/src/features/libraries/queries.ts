import { useQuery } from "@tanstack/react-query";

import { getLibrary, listLibraries } from "./api";

export const librariesQueryKey = ["libraries"] as const;

export function libraryQueryKey(libraryId: string) {
  return ["libraries", libraryId] as const;
}

export function useLibraries() {
  return useQuery({
    queryKey: librariesQueryKey,
    queryFn: ({ signal }) => listLibraries(signal),
  });
}

export function useLibrary(libraryId: string) {
  return useQuery({
    enabled: libraryId.length > 0,
    queryKey: libraryQueryKey(libraryId),
    queryFn: ({ signal }) => getLibrary(libraryId, signal),
  });
}
