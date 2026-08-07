import { useQuery } from "@tanstack/react-query";

import { listMembers } from "./api";

export function membersQueryKey(libraryId: string) {
  return ["libraries", libraryId, "members"] as const;
}

export function useMembers(libraryId: string) {
  return useQuery({
    queryKey: membersQueryKey(libraryId),
    queryFn: ({ signal }) => listMembers(libraryId, signal),
  });
}
