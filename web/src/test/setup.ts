import "@testing-library/jest-dom/vitest";
import { transferableAbortController } from "node:util";
import { cleanup } from "@testing-library/react";
import { afterAll, afterEach, beforeAll } from "vitest";

import i18n from "../i18n";
import { server } from "./server";

// Node's fetch implementation rejects jsdom's cross-realm signal objects.
const nativeAbortController = transferableAbortController();
Object.defineProperties(globalThis, {
  AbortController: { configurable: true, value: nativeAbortController.constructor },
  AbortSignal: { configurable: true, value: nativeAbortController.signal.constructor },
});

beforeAll(() => {
  server.listen({ onUnhandledRequest: "error" });
});

afterEach(async () => {
  cleanup();
  server.resetHandlers();
  document.cookie = "folioharbor_csrf=; Max-Age=0; Path=/";
  document.documentElement.lang = "en";
  await i18n.changeLanguage("en");
});

afterAll(() => {
  server.close();
});
