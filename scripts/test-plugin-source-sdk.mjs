// Build the distributed SDK outside this repository; no app Rust path dependency is available.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";

const binary = path.resolve(
  process.env.PLUGIN_SOURCE_BINARY ?? "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-source-sdk-")),
);
const sdk = path.join(root, "sdk");
const hash = (file) =>
  createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report = {
  complete: false,
  binarySha256: hash(binary),
  cases: [],
  realModelCalls: 0,
};
async function run(command, args, cwd = root) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "",
      stderr = "";
    const timeout = setTimeout(() => child.kill("SIGKILL"), 300_000);
    child.stdout.on("data", (b) => {
      stdout += b;
    });
    child.stderr.on("data", (b) => {
      stderr = (stderr + b).slice(-8000);
    });
    child.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.on("exit", (code) => {
      clearTimeout(timeout);
      code === 0
        ? resolve(stdout)
        : reject(Error(`${command} exited ${code}: ${stderr}`));
    });
  });
}
try {
  fs.cpSync(path.resolve("sdk/plugins"), sdk, {
    recursive: true,
    filter: (file) =>
      !path
        .relative(path.resolve("sdk/plugins"), file)
        .split(path.sep)
        .includes("target"),
  });
  const lock = hash(path.join(sdk, "Cargo.lock"));
  const metadata = JSON.parse(
    await run(
      "cargo",
      ["+1.92.0", "metadata", "--no-deps", "--format-version", "1"],
      sdk,
    ),
  );
  assert.equal(fs.realpathSync(metadata.workspace_root), sdk);
  assert(
    metadata.packages.every((p) => p.manifest_path.startsWith(sdk + path.sep)),
  );
  assert(
    metadata.packages.every((p) =>
      p.dependencies.every((d) => !d.path || d.path.startsWith(sdk + path.sep)),
    ),
  );
  const component = path.join(sdk, "examples/full-history/package/plugin.wasm");
  fs.rmSync(component, { force: true });
  await run(
    "cargo",
    [
      "+1.92.0",
      "build",
      "--locked",
      "--offline",
      "--release",
      "--target",
      "wasm32-unknown-unknown",
      "-p",
      "full-history-plugin",
    ],
    sdk,
  );
  assert.equal(hash(path.join(sdk, "Cargo.lock")), lock);
  const wasm = path.join(
    sdk,
    "target/wasm32-unknown-unknown/release/full_history_plugin.wasm",
  );
  await run(binary, ["--plugin-sdk", "componentize", wasm, component]);
  const key = path.join(root, "publisher.key");
  await run(binary, ["--plugin-sdk", "keygen", key]);
  const archive = path.join(root, "independent.laplug");
  await run(binary, [
    "--plugin-sdk",
    "pack",
    path.dirname(component),
    archive,
    key,
  ]);
  const checked = JSON.parse(
    await run(binary, ["--plugin-sdk", "check", archive]),
  );
  assert.equal(checked.manifest.manifestVersion, 1);
  assert.equal(checked.manifest.id, "org.example.full-history");
  report.cases.push(
    "copied SDK builds the history guest with its unchanged lockfile and no application path dependencies; shipped utility componentizes, signs and validates the resulting package",
  );
  report.wasmSha256 = hash(wasm);
  report.componentSha256 = hash(component);
  report.archiveSha256 = hash(archive);
  report.sdkLockSha256 = lock;
  report.complete = true;
} finally {
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.PLUGIN_SOURCE_REPORT)
    fs.writeFileSync(
      process.env.PLUGIN_SOURCE_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
