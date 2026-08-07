import { execFileSync } from "node:child_process";
import { rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const repositoryRoot = resolve(import.meta.dirname, "../..");
const composeFile = resolve(repositoryRoot, "tests/e2e/compose.test.yaml");
const statusFile = resolve(repositoryRoot, "tests/e2e/compose-status.txt");
const composeArguments = ["compose", "-p", "folioharbor-e2e", "-f", composeFile];

function docker(arguments_: string[], quiet = false): string {
  return execFileSync("docker", arguments_, {
    cwd: repositoryRoot,
    encoding: "utf8",
    env: { ...process.env, DOCKER_BUILDKIT: "1" },
    stdio: quiet ? ["ignore", "pipe", "pipe"] : ["ignore", "inherit", "inherit"],
  });
}

function down(): void {
  try {
    docker([...composeArguments, "down", "-v", "--remove-orphans"], true);
  } catch {
    // Teardown is best-effort after the release-gate failure has already been preserved.
  }
}

function captureStatus(message?: string): void {
  let rows = "";
  try {
    rows = docker([
      ...composeArguments,
      "ps",
      "--all",
      "--format",
      "{{.Service}}\t{{.State}}\t{{.Health}}\t{{.ExitCode}}",
    ], true);
  } catch {
    // A build or registry failure can occur before Compose creates a project.
  }
  writeFileSync(
    statusFile,
    ["service\tstate\thealth\texit_code", rows.trim(), message ?? ""]
      .filter((line) => line.length > 0)
      .join("\n") + "\n",
    { mode: 0o600 },
  );
}

export default function globalSetup(): () => void {
  rmSync(statusFile, { force: true });
  down();
  try {
    if (process.env.FOLIOHARBOR_E2E_SKIP_BUILD === "1") {
      docker(["image", "inspect", "folioharbor-e2e-app:local"], true);
    } else {
      docker([
        "build",
        "--quiet",
        "--tag",
        "folioharbor-e2e-app:local",
        "--file",
        resolve(repositoryRoot, "tests/e2e/Dockerfile"),
        repositoryRoot,
      ]);
    }
    docker([...composeArguments, "up", "--no-build", "--wait"]);
  } catch (error) {
    captureStatus("Compose setup failed; inspect the non-artifacted job output");
    down();
    throw error;
  }

  return () => {
    captureStatus();
    down();
  };
}
