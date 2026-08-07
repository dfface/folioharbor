import { useQuery } from "@tanstack/react-query";

import { libraryQueryKey } from "../libraries/queries";
import { listMembers } from "./api";

export function membersQueryKey(libraryId: string) {
  return [...libraryQueryKey(libraryId), "members"] as const;
}

export function useMembers(libraryId: string, enabled = true) {
  return useQuery({
    enabled,
    queryKey: membersQueryKey(libraryId),
    queryFn: ({ signal }) => listMembers(libraryId, signal),
  });
}
