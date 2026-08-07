import { spawnSync } from "node:child_process";

import type { APIResponse } from "@playwright/test";

const fixedFailure = "E2E redaction gate detected sensitive data";
const prohibitedPatterns = [
  /folioharbor_(?:session|csrf)=/iu,
  /(?:^|\s)(?:set-cookie|cookie)\s*:/imu,
  /\/var\/lib\/folioharbor/iu,
  /postgres(?:ql)?:\/\/[^\s"']+/iu,
  /(?:storage_key|sha256|blob:[^\s"'<>;]+:[^\s"'<>;]+)/iu,
] as const;
const composePrefix = [
  "compose",
  "-p",
  "folioharbor-e2e",
  "-f",
  "../tests/e2e/compose.test.yaml",
] as const;

function variants(secret: string): string[] {
  const encoded = Buffer.from(secret, "utf8");
  const values = new Set([
    secret,
    encodeURIComponent(secret),
    encoded.toString("base64"),
    encoded.toString("base64url"),
  ]);
  if (/^[\da-f]{64}$/iu.test(secret)) {
    const digest = Buffer.from(secret, "hex");
    values.add(secret.toUpperCase());
    values.add(digest.toString("base64"));
    values.add(digest.toString("base64url"));
  }
  return [...values].filter((value) => value.length >= 8);
}

export class SensitiveCaptureGate {
  readonly #captures: string[] = [];
  readonly #sentinels = new Set<string>();

  constructor(sentinels: readonly string[] = []) {
    this.addSentinels(sentinels);
  }

  addSentinels(sentinels: readonly string[]): void {
    for (const sentinel of sentinels) {
      for (const variant of variants(sentinel)) {
        this.#sentinels.add(variant);
      }
    }
  }

  capture(_label: string, content: string | Buffer): void {
    this.#captures.push(typeof content === "string" ? content : content.toString("utf8"));
  }

  async captureResponse(label: string, response: APIResponse): Promise<void> {
    this.capture(`${label} headers`, JSON.stringify(response.headersArray()));
    this.capture(`${label} body`, await response.body());
  }

  assertSafe(): void {
    const leaked = this.#captures.some((capture) =>
      prohibitedPatterns.some((pattern) => pattern.test(capture)) ||
      [...this.#sentinels].some((sentinel) => capture.includes(sentinel))
    );
    if (leaked) {
      throw new Error(fixedFailure);
    }
  }
}

function dockerCompose(arguments_: readonly string[]): string {
  const result = spawnSync("docker", [...composePrefix, ...arguments_], {
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error("E2E security capture command failed");
  }
  return result.stdout;
}

export function capturedServiceLogs(): string {
  return dockerCompose(["logs", "--no-color", "api", "worker"]);
}

export function storageKeyForUpload(uploadId: string): string {
  if (!/^[\da-f]{8}-(?:[\da-f]{4}-){3}[\da-f]{12}$/iu.test(uploadId)) {
    throw new Error("E2E security fixture has an invalid upload identifier");
  }
  const output = dockerCompose([
    "exec",
    "-T",
    "postgres",
    "psql",
    "--username",
    "postgres",
    "--dbname",
    "folioharbor",
    "--tuples-only",
    "--no-align",
    "--command",
    `SELECT storage_key FROM folioharbor.upload_sessions WHERE upload_id='${uploadId}'::uuid`,
  ]).trim();
  if (output.length === 0) {
    throw new Error("E2E security fixture storage key is unavailable");
  }
  return output;
}
