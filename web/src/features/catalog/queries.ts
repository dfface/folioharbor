import { useInfiniteQuery, useQuery } from "@tanstack/react-query";

import { libraryQueryKey } from "../libraries/queries";
import { getItem, listBooks } from "./api";

export function booksQueryKey(libraryId: string) {
  return [...libraryQueryKey(libraryId), "books"] as const;
}

export function useBooks(libraryId: string) {
  return useInfiniteQuery({
    initialPageParam: null as string | null,
    queryKey: booksQueryKey(libraryId),
    queryFn: ({ pageParam, signal }) => listBooks(libraryId, pageParam, signal),
    getNextPageParam: (lastPage) => lastPage.next_cursor ?? undefined,
  });
}

export function useItem(libraryId: string, itemId: string) {
  return useQuery({
    enabled: libraryId.length > 0 && itemId.length > 0,
    queryKey: [...libraryQueryKey(libraryId), "items", itemId],
    queryFn: ({ signal }) => getItem(libraryId, itemId, signal),
  });
}
