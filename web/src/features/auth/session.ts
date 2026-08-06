import { useQuery } from "@tanstack/react-query";

import { ApiProblem } from "../../api/problem";
import { currentSession, type Session } from "./api";

export const sessionQueryKey = ["auth", "session"] as const;

async function loadSession(signal: AbortSignal): Promise<Session | null> {
  try {
    return await currentSession(signal);
  } catch (error) {
    if (error instanceof ApiProblem && error.problem.status === 401) {
      return null;
    }
    throw error;
  }
}

type SessionState =
  | { status: "anonymous" }
  | { status: "authenticated"; session: Session }
  | { status: "loading" };

export function useSession(): SessionState {
  const query = useQuery({
    queryFn: ({ signal }) => loadSession(signal),
    queryKey: sessionQueryKey,
    retry: false,
    staleTime: 30_000,
  });

  if (query.isPending) {
    return { status: "loading" };
  }
  if (query.data == null) {
    return { status: "anonymous" };
  }
  return { session: query.data, status: "authenticated" };
}
