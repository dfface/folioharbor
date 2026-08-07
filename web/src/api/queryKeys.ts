export const authenticatedResourcesQueryKey = ["authenticated"] as const;
export const librariesQueryKey = [...authenticatedResourcesQueryKey, "libraries"] as const;
