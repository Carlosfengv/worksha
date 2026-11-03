import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { parseArgs } from "node:util";

const { values } = parseArgs({
  options: { baseline: { type: "string" }, report: { type: "string" } },
});
assert.ok(["darwin", "linux"].includes(process.platform), "Unix WAL fixture");
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--locked", "--format-version=1"], {
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  }),
);
const dependency = metadata.packages.find((p) => p.name === "libsqlite3-sys");
assert.ok(dependency);
const source = path.join(
  path.dirname(dependency.manifest_path),
  "sqlite3/sqlite3.c",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "sqlite-wal-lock-")),
);
const results = [];
const report = { complete: false, results };

async function exercise(name, amalgamation, deadlockExpected) {
  const bytes = fs.readFileSync(amalgamation);
  const version = bytes
    .toString()
    .match(/^#define SQLITE_VERSION\s+"([^"]+)"/m)?.[1];
  const executable = path.join(root, name);
  execFileSync(process.env.CC ?? "clang", [
    "-O0",
    "-g",
    "-DSQLITE_THREADSAFE=1",
    `-DLOCAL_AGENT_SQLITE_SOURCE=${JSON.stringify(amalgamation)}`,
    "scripts/fixtures/sqlite-wal-lock.c",
    "-o",
    executable,
    "-lpthread",
    "-lm",
  ]);
  const trials = [];
  for (let n = 0; n < 5; n++) {
    const file = path.join(root, `${name}-${n}.sqlite3`);
    const child = spawn(executable, [file], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "",
      stderr = "",
      timedOut = false;
    child.stdout.on("data", (b) => {
      stdout += b;
    });
    child.stderr.on("data", (b) => {
      stderr += b;
    });
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, 3000);
    const [code, signal] = await once(child, "exit");
    clearTimeout(timer);
    trials.push({ timedOut, code, signal });
    assert.equal(timedOut, deadlockExpected, `${name}: ${stderr}`);
    if (!deadlockExpected) {
      assert.equal(code, 0, stderr);
      assert.equal(stdout.trim(), version);
    }
  }
  results.push({
    name,
    version,
    sourceSha256: createHash("sha256").update(bytes).digest("hex"),
    deadlockExpected,
    trials,
  });
}

try {
  if (values.baseline)
    await exercise("baseline", path.resolve(values.baseline), true);
  await exercise("candidate", source, false);
  assert.equal(results.at(-1).version, "3.51.3");
  report.complete = true;
  console.log(
    "Unix WAL close/open: deterministic lock schedule passed five times; stored value preserved.",
  );
} finally {
  fs.rmSync(root, { recursive: true, force: true });
  if (values.report)
    fs.writeFileSync(values.report, JSON.stringify(report, null, 2) + "\n");
}
