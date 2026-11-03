import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const binarySha256 = createHash("sha256")
  .update(readFileSync(binary))
  .digest("hex");
const root = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-plugin-records-")),
);
const data = path.join(root, "data");
const home = path.join(root, "source");
mkdirSync(data);
mkdirSync(home);
const target = path.join(home, "selected.json");
writeFileSync(target, '{"preserve":42}\n', { mode: 0o600 });
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const original = hash(readFileSync(target));
const token = randomUUID();
const query = { pluginId: "org.example.removed", digest: "a".repeat(64) };
const report = {
  complete: false,
  binarySha256,
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child;
let base;
let db;

async function start() {
  child = spawn(binary, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.resume();
  const lines = createInterface({ input: child.stdout });
  let timer;
  const ready = await Promise.race([
    once(lines, "line").then(([line]) => JSON.parse(line)),
    once(child, "exit").then(() => {
      throw Error("isolated service exited before readiness");
    }),
    new Promise((_, reject) => {
      timer = setTimeout(
        () => reject(Error("isolated service readiness timed out")),
        15000,
      );
    }),
  ]).finally(() => clearTimeout(timer));
  base = `http://127.0.0.1:${ready.port}`;
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit");
  child.kill("SIGINT");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  await done.finally(() => clearTimeout(timer));
}
async function api(route, body, method = body === undefined ? "GET" : "POST") {
  const response = await fetch(base + route, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(5000),
  });
  return { status: response.status, body: await response.json() };
}
const history = () =>
  api("/api/plugin-extensions/status", { pluginId: "", digest: "" });
const passed = (name) => {
  report.cases.push(name);
  console.log("通过：" + name);
};
function unreadable(result) {
  assert.equal(result.status, 503);
  assert.equal(result.body.code, "invalid_record");
  assert.match(result.body.message, /保留原文件与备份/);
  assert.equal(
    JSON.stringify(result).includes("private-record-sentinel"),
    false,
  );
  assert.equal(Object.hasOwn(result.body, "plans"), false);
}

try {
  await start();
  const empty = await history();
  assert.equal(empty.status, 200);
  assert.deepEqual(empty.body.plans, []);
  passed("confirmed empty history remains a successful empty result");
  db = new DatabaseSync(path.join(data, "plugin-extensions.sqlite3"));
  const malformed = "{private-record-sentinel";
  db.prepare("INSERT INTO plans VALUES(?,?,?)").run(
    "broken",
    query.pluginId,
    malformed,
  );
  unreadable(await history());
  const restore = await api("/api/plugin-extensions/action", {
    ...query,
    action: { kind: "restore-config", operationId: "broken" },
  });
  unreadable(restore);
  assert.equal(
    db.prepare("SELECT record FROM plans WHERE id='broken'").get().record,
    malformed,
  );
  assert.equal(hash(readFileSync(target)), original);
  passed(
    "damaged history and direct restore report unreadable metadata without rewriting files or exposing contents",
  );
  for (const route of ["/api/snapshot", "/api/plugins"])
    assert.equal((await api(route)).status, 200);
  passed(
    "optional record failure preserves the core snapshot and ordinary plugin directory",
  );
  await stop();
  await start();
  unreadable(await history());
  assert.equal(
    db.prepare("SELECT record FROM plans WHERE id='broken'").get().record,
    malformed,
  );
  passed("restart preserves damaged records and their error state");
  const snapshot = await api("/api/snapshot");
  assert.equal(snapshot.status, 200);
  const metadata = statSync(target, { bigint: true });
  const stamp = {
    digest: original,
    inode: Number(metadata.ino),
    device: Number(metadata.dev),
    mode: Number(metadata.mode & 0o777n),
    modified_sec: Number(metadata.mtimeNs / 1000000000n),
    modified_nsec: Number(metadata.mtimeNs % 1000000000n),
  };
  const lease = {
    id: query.pluginId,
    digest: query.digest,
    epoch: "1",
    revision: "1",
  };
  const valid = {
    id: "valid-plan",
    plugin_id: query.pluginId,
    digest: query.digest,
    lease,
    grant: {
      lease,
      path: target,
      device: stamp.device,
      inode: stamp.inode,
      hash: original,
      executable: null,
    },
    provider: snapshot.body.providers[0],
    edits: [],
    credential_pointers: [],
    stamp,
    state: "prepared",
    created_at: "2026-10-27T00:00:00Z",
    after_hash: null,
    backup: null,
    backup_ref: null,
    after_stamp: null,
  };
  db.prepare("INSERT INTO plans VALUES(?,?,?)").run(
    valid.id,
    query.pluginId,
    JSON.stringify(valid),
  );
  unreadable(await history());
  assert.equal(
    db.prepare("SELECT count(*) AS count FROM plans").get().count,
    2,
  );
  passed(
    "mixed healthy and damaged records cannot masquerade as a complete successful list",
  );
  // Remove only our injected fault; the product never deletes an unreadable row.
  db.exec("DELETE FROM plans WHERE id='broken'");
  const recovered = await history();
  assert.equal(recovered.status, 200);
  assert.equal(recovered.body.plans.length, 1);
  assert.equal(recovered.body.plans[0].planId, valid.id);
  passed(
    "an explicitly repaired fixture can be read again without restarting or replaying a write",
  );
  db.exec("ALTER TABLE plans RENAME TO preserved_plans");
  const unavailable = await history();
  assert.notEqual(unavailable.status, 200);
  assert.equal(unavailable.body.code, "unavailable");
  assert.equal((await api("/api/snapshot")).status, 200);
  db.exec("ALTER TABLE preserved_plans RENAME TO plans");
  assert.equal(hash(readFileSync(target)), original);
  passed("SQL query failure remains distinct from a confirmed empty history");
  report.complete = true;
} finally {
  db?.close();
  await stop();
  rmSync(root, { recursive: true, force: true });
  report.ownedServiceStopped = true;
  report.temporaryDataRemoved = true;
  if (process.env.PLUGIN_RECORDS_REPORT) {
    const targetReport = path.resolve(process.env.PLUGIN_RECORDS_REPORT);
    mkdirSync(path.dirname(targetReport), { recursive: true });
    writeFileSync(targetReport, JSON.stringify(report, null, 2) + "\n");
  }
}
