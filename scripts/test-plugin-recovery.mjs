import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  cpSync,
  linkSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";
import { readCoreSchema } from "./lib/core-schema.mjs";

// Optional stores must never prevent access to the original data. All files,
// future schemas, and linked targets below are disposable synthetic fixtures.
const binary = path.resolve(
  process.env.PLUGIN_TEST_BINARY ?? "target/debug/local-agent-service",
);
const root = realpathSync(mkdtempSync(path.join(tmpdir(), "plugin-recovery-")));
const report = { complete: false, binary, root, cases: [], realModelCalls: 0 };
const digest = (file) =>
  createHash("sha256").update(readFileSync(file)).digest("hex");
const save = () => {
  if (process.env.PLUGIN_RECOVERY_REPORT)
    writeFileSync(
      process.env.PLUGIN_RECOVERY_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
};
async function start(directory) {
  const home = path.join(directory, "home");
  mkdirSync(home, { recursive: true });
  const token = randomUUID();
  const env = {
    ...process.env,
    PATH: "/usr/bin:/bin:/usr/sbin:/sbin",
    LOCAL_AGENT_SOURCE_HOME: home,
    LOCAL_AGENT_SERVICE_TOKEN: token,
  };
  delete env.LOCAL_AGENT_TEST_MEMORY_VAULT;
  const child = spawn(binary, ["--data-dir", directory], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const exited = once(child, "exit");
  let stderr = "";
  child.stderr.on("data", (bytes) => {
    stderr += bytes;
  });
  const lines = createInterface({ input: child.stdout });
  let base;
  try {
    const [line] = await Promise.race([
      once(lines, "line", { signal: AbortSignal.timeout(10000) }),
      exited.then(() => {
        throw Error(`Service exited before ready: ${stderr}`);
      }),
    ]);
    base = `http://127.0.0.1:${JSON.parse(line).port}`;
  } catch (error) {
    if (child.exitCode === null) {
      child.kill("SIGKILL");
      await exited;
    }
    throw error;
  }
  return {
    async request(route, body) {
      const response = await fetch(base + route, {
        method: body === undefined ? "GET" : "POST",
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(5000),
      });
      return { status: response.status, value: await response.json() };
    },
    async stop() {
      if (child.exitCode !== null) return;
      try {
        await this.request("/api/runtime/shutdown", {});
        await exited;
      } finally {
        if (child.exitCode === null) {
          child.kill("SIGKILL");
          await exited;
        }
      }
    },
  };
}
function future(file, table) {
  const db = new DatabaseSync(file);
  db.exec(
    `CREATE TABLE ${table}(version INTEGER NOT NULL); INSERT INTO ${table} VALUES(99); CREATE TABLE future_sentinel(value TEXT); INSERT INTO future_sentinel VALUES('preserve');`,
  );
  db.close();
}
try {
  const baseline = path.join(root, "baseline");
  mkdirSync(baseline);
  const initial = await start(baseline);
  const original = (await initial.request("/api/snapshot")).value;
  const capabilities = await initial.request("/api/plugins");
  assert.equal(capabilities.status, 200);
  assert.equal(capabilities.value.apiVersion, "1.0.0");
  assert.equal(capabilities.value.externalProtocol, "explicit-binding");
  assert.equal(capabilities.value.dsh, "unverified");
  await initial.stop();
  const coreSchema = readCoreSchema(path.join(baseline, "local-agent.sqlite3"));
  assert(coreSchema.version > 0);
  for (const name of [
    "corrupt-basic",
    "future-basic",
    "linked-basic",
    "invalid-basic-selection",
    "blocked-plugin-directory",
    "corrupt-extensions",
    "future-extensions",
  ]) {
    const data = path.join(root, name);
    cpSync(baseline, data, { recursive: true });
    const extensions = name.endsWith("extensions");
    const file = path.join(
      data,
      extensions ? "plugin-extensions.sqlite3" : "plugin-host.sqlite3",
    );
    let watched = file;
    if (name === "blocked-plugin-directory") {
      rmSync(path.join(data, "plugins"), { recursive: true });
      writeFileSync(path.join(data, "plugins"), "preserve non-directory");
      watched = path.join(data, "plugins");
    } else if (name === "invalid-basic-selection") {
      const db = new DatabaseSync(file);
      db.prepare(
        "INSERT INTO packages(digest,plugin_id,version,manifest,bytes) VALUES(?,?,?,?,?)",
      ).run("a".repeat(64), "org.example.damaged", "1.0.0", "{invalid", 0);
      db.close();
    } else {
      rmSync(file, { force: true });
      if (name.startsWith("future"))
        future(file, extensions ? "extension_version" : "host_version");
      else if (name === "linked-basic") {
        watched = path.join(root, "linked-target.sqlite3");
        future(watched, "foreign_version");
        linkSync(watched, file);
      } else writeFileSync(file, "not a SQLite database\n");
    }
    const before = digest(watched);
    const service = await start(data);
    try {
      const snapshot = await service.request("/api/snapshot");
      assert.equal(snapshot.status, 200, name);
      assert.deepEqual(snapshot.value.providers, original.providers, name);
      assert.deepEqual(snapshot.value.settings, original.settings, name);
      assert.equal((await service.request("/api/sessions")).status, 200, name);
      const optional = await service.request(
        extensions ? "/api/plugin-extensions/status" : "/api/plugins",
        extensions ? { pluginId: "", digest: "" } : undefined,
      );
      assert.notEqual(optional.status, 200, name);
      assert.equal(
        optional.value.code,
        name === "future-extensions" ? "incompatible_api" : "unavailable",
        name,
      );
      assert.equal((await service.request("/api/runtime")).status, 200, name);
    } finally {
      await service.stop();
    }
    if (name !== "invalid-basic-selection")
      assert.equal(digest(watched), before, name);
    const core = new DatabaseSync(path.join(data, "local-agent.sqlite3"), {
      readOnly: true,
    });
    assert.deepEqual(readCoreSchema(path.join(data, "local-agent.sqlite3")), coreSchema, name);
    assert.equal(
      core.prepare("PRAGMA integrity_check").get().integrity_check,
      "ok",
    );
    core.close();
    report.cases.push({
      name,
      originalFeaturesAvailable: true,
      optionalFailureExplicit: true,
      originalDataPreserved: true,
    });
    save();
  }
  report.complete = true;
  save();
  console.log(
    "Optional plugin store failure isolation and future-schema preservation passed.",
  );
} catch (error) {
  report.error = error.message;
  save();
  throw error;
}
