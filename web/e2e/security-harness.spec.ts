import { expect, test } from "@playwright/test";

import { SensitiveCaptureGate } from "./security-scan";

test("redaction gate rejects an injected sentinel without echoing it", () => {
  const sentinel = `token-${crypto.randomUUID()}-must-not-appear`;
  const gate = new SensitiveCaptureGate([sentinel]);
  gate.capture("injected security harness", `prefix:${sentinel}:suffix`);

  let diagnostic = "";
  try {
    gate.assertSafe();
  } catch (error) {
    diagnostic = error instanceof Error ? error.message : "non-error diagnostic";
  }

  expect(diagnostic).toBe("E2E redaction gate detected sensitive data");
  expect(diagnostic.includes(sentinel)).toBe(false);
});

test("redaction gate permits the reader CSP scheme but rejects an opaque storage key", () => {
  const safe = new SensitiveCaptureGate();
  safe.capture("reader policy", "img-src 'self' data: blob:; script-src 'none'");
  expect(() => {
    safe.assertSafe();
  }).not.toThrow();

  const unsafe = new SensitiveCaptureGate();
  unsafe.capture("storage marker", `blob:instance-v1:${crypto.randomUUID()}:42`);
  expect(() => {
    unsafe.assertSafe();
  }).toThrow("E2E redaction gate detected sensitive data");
});
