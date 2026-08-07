import type { components } from "../../api/generated";

export type Locator = components["schemas"]["Locator"];

interface LocatorInput {
  href: string;
  mediaType?: string;
  position: number;
  progression?: number;
  totalProgression: number;
}

export function createLocator({
  href,
  mediaType,
  position,
  progression = 0,
  totalProgression,
}: LocatorInput): Locator {
  const locator: Locator = {
    href,
    locations: { position, progression, totalProgression },
    extensions: { version: 1, values: {} },
  };
  if (mediaType !== undefined) {
    locator.type = mediaType;
  }
  return locator;
}
