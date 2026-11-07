// Optional-store failures must preserve bytes and leave native reads available.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { readCoreSchema } from "./lib/core-schema.mjs";

const option = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i < 0 ? fallback : process.argv[i + 1];
};
const binary = fs.realpathSync(
  option("--binary", "target/debug/local-agent-service"),
);
const output = path.resolve(
  option("--report", ".work/domain-store-isolation.json"),
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-domain-store-")),
);
const home = path.join(root, "home");
const hash = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const report = {
  complete: false,
  phaseAccepted: false,
  binarySha256: hash(binary),
  harnessSha256: hash(import.meta.filename),
  root,
  cases: [],
  failures: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
  scope:
    "Stopped owned database copies only; corrupt/future/foreign/read-only/linked/directory targets and explicit repair. No disk-full simulation or automatic repair.",
};
let child,
  base,
  token,
  active = 0;
const save = () =>
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
const delay = (ms) => new Promise((r) => setTimeout(r, ms));
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const owned = child,
    done = once(owned, "exit");
  owned.kill("SIGTERM");
  let timer;
  try {
    await Promise.race([
      done,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          owned.kill("SIGKILL");
          reject(Error("owned service stop deadline"));
        }, 8000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
async function request(
  route,
  body,
  method = body === undefined ? "GET" : "POST",
) {
  active++;
  try {
    const response = await fetch(base + route, {
      method,
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(10000),
    });
    return { status: response.status, value: await response.json() };
  } finally {
    active--;
  }
}
async function api(...args) {
  const result = await request(...args);
  assert.equal(result.status, 200, JSON.stringify(result));
  return result.value;
}
async function boot(data) {
  token = randomUUID();
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
  let error = "";
  child.stderr.on("data", (b) => {
    error = (error + b).slice(-4000);
  });
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(20000),
    }),
    once(child, "exit").then(() => {
      throw Error(error);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: true },
    "PUT",
  );
  const deadline = Date.now() + 20000;
  while (
    !(await api("/api/snapshot")).sessionSources.some(
      (s) => s.agentId === "claude" && s.status === "ready",
    )
  ) {
    assert(Date.now() < deadline, "native source ready deadline");
    await delay(30);
  }
}
function database(p, sql) {
  const db = new DatabaseSync(p);
  try {
    db.exec(sql);
  } finally {
    db.close();
  }
}
function fingerprint(p) {
  const st = fs.lstatSync(p);
  if (st.isSymbolicLink())
    return { kind: "link", destination: fs.readlinkSync(p) };
  if (st.isDirectory())
    return {
      kind: "directory",
      files: Object.fromEntries(
        fs
          .readdirSync(p)
          .sort()
          .map((n) => [n, fingerprint(path.join(p, n))]),
      ),
    };
  return { kind: "file", size: st.size, sha256: hash(p) };
}
const scope = { kind: "builtin", sourceIds: ["claude"] };
const unifiedQuery = {
  scope,
  filter: { productId: null, query: null, days: null },
  cursor: null,
  snapshotToken: null,
};
const optionalFiles = [
  { file: "domain-host.sqlite3", table: "domain_version", label: "basic-domain" },
  { file: "plugin-source-host.sqlite3", table: "full_version", label: "history" },
];
const faults = option(
  "--faults",
  "corrupt,future,foreign,read-only,hardlink,symlink,directory",
).split(",");
assert(
  faults.every((s) =>
    [
      "corrupt",
      "future",
      "foreign",
      "read-only",
      "hardlink",
      "symlink",
      "directory",
    ].includes(s),
  ),
);
try {
  const native = path.join(home, ".claude/projects/owned/session.jsonl");
  fs.mkdirSync(path.dirname(native), { recursive: true });
  fs.writeFileSync(
    native,
    [
      {
        type: "user",
        sessionId: "store-native",
        uuid: "u",
        entrypoint: "cli",
        timestamp: "2026-10-31T01:00:00Z",
        message: { role: "user", content: "owned native session" },
      },
      {
        type: "assistant",
        sessionId: "store-native",
        uuid: "a",
        timestamp: "2026-10-31T01:00:01Z",
        message: {
          id: "r",
          model: "fixture",
          content: [{ type: "text", text: "owned body" }],
          usage: { input_tokens: 3, output_tokens: 1 },
        },
      },
    ]
      .map(JSON.stringify)
      .join("\n") + "\n",
  );
  const nativeHash = hash(native),
    baseline = path.join(root, "baseline");
  fs.mkdirSync(baseline);
  const key = path.join(root, "key"),
    archive = path.join(root, "aurora.laplug");
  for (const args of [
    ["keygen", key],
    [
      "pack",
      path.resolve("sdk/plugins/examples/aurora-agent/package"),
      archive,
      key,
    ],
  ])
    execFileSync(binary, ["--plugin-sdk", ...args], {
      timeout: 30000,
      stdio: "pipe",
    });
  await boot(baseline);
  const builtinProducts = (await api("/api/domain/directory")).catalog.products;
  assert.equal(builtinProducts.length, 5);
  const review = await api("/api/plugins/inspect", { path: archive });
  await api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
  const original = await api("/api/snapshot");
  const normalDirectory = await api("/api/domain/directory");
  assert.equal(normalDirectory.externalProblem, null);
  const normalSessions = await api("/api/sessions?agent=claude");
  assert.equal(normalSessions.total, 1);
  const normalUnified = await api("/api/domain/sessions", unifiedQuery);
  assert.equal(normalUnified.items.length, 1);
  const legacy = await api("/api/plugins");
  await stop();
  database(
    path.join(baseline, "domain-host.sqlite3"),
    "CREATE TABLE domain_version(version INTEGER NOT NULL); INSERT INTO domain_version VALUES(1); CREATE TABLE source_consents(plugin_id TEXT PRIMARY KEY,record TEXT NOT NULL); CREATE TABLE source_projections(plugin_id TEXT PRIMARY KEY,record TEXT NOT NULL);",
  );
  const originalSchema = readCoreSchema(
    path.join(baseline, "local-agent.sqlite3"),
  );
  assert.equal(originalSchema.version, 16);
  for (const store of optionalFiles)
    for (const fault of faults) {
      const name = `${store.label}-${fault}`,
        data = path.join(root, name);
      fs.cpSync(baseline, data, { recursive: true });
      const target = path.join(data, store.file),
        watched = [];
      if (fault === "read-only") fs.chmodSync(target, 0o400);
      else {
        fs.rmSync(target);
        if (fault === "corrupt")
          fs.writeFileSync(target, "not a SQLite database\n");
        else if (fault === "future")
          database(
            target,
            `CREATE TABLE ${store.table}(version INTEGER); INSERT INTO ${store.table} VALUES(99); CREATE TABLE future_sentinel(value TEXT); INSERT INTO future_sentinel VALUES('preserve');`,
          );
        else if (fault === "foreign")
          database(
            target,
            "CREATE TABLE foreign_sentinel(value TEXT); INSERT INTO foreign_sentinel VALUES('preserve');",
          );
        else if (["hardlink", "symlink"].includes(fault)) {
          const foreign = path.join(root, `${name}-target.sqlite3`);
          database(
            foreign,
            "CREATE TABLE foreign_sentinel(value TEXT); INSERT INTO foreign_sentinel VALUES('preserve');",
          );
          if (fault === "hardlink") fs.linkSync(foreign, target);
          else fs.symlinkSync(foreign, target);
          watched.push(foreign);
        } else {
          fs.mkdirSync(target);
          fs.writeFileSync(path.join(target, "notes"), "preserve owned note\n");
        }
      }
      watched.push(target);
      const before = watched.map(fingerprint);
      const result = { name, expectedOptionalFailure: true };
      try {
        await boot(data);
        const snapshot = await api("/api/snapshot");
        assert.deepEqual(snapshot.providers, original.providers);
        assert.deepEqual(snapshot.settings, original.settings);
        const sessions = await api("/api/sessions?agent=claude");
        assert.deepEqual(sessions.items, normalSessions.items);
        assert.deepEqual(sessions.summary, normalSessions.summary);
        const unified = await api("/api/domain/sessions", unifiedQuery);
        assert.deepEqual(unified.items, normalUnified.items);
        assert.deepEqual(
          unified.sourceSummaries,
          normalUnified.sourceSummaries,
        );
        assert.deepEqual((await api("/api/plugins")).items, legacy.items);
        await api("/api/runtime");
        const directory = await api("/api/domain/directory");
        const catalog = await api("/api/domain/capabilities");
        result.externalProblem = directory.externalProblem;
        result.capabilityProblem = catalog.externalProblem;
        result.nativeReadsAvailable = true;
        const builtinIds = new Set(builtinProducts.map((product) => product.id));
        assert.deepEqual(
          directory.catalog.products.filter((product) => builtinIds.has(product.id)),
          builtinProducts,
        );
        if (store.label === "history")
          assert.deepEqual(directory.catalog.products, normalDirectory.catalog.products);
        assert(
          directory.externalProblem,
          `${name}: optional store silently accepted`,
        );
        assert(
          catalog.externalProblem,
          `${name}: optional capability failure missing`,
        );
      } catch (error) {
        result.error = error.stack;
      } finally {
        await stop();
      }
      result.originalTargetPreserved =
        JSON.stringify(before) === JSON.stringify(watched.map(fingerprint));
      result.nativeSourcePreserved = hash(native) === nativeHash;
      assert(result.nativeSourcePreserved);
      assert.deepEqual(
        readCoreSchema(path.join(data, "local-agent.sqlite3")),
        originalSchema,
      );
      const core = new DatabaseSync(path.join(data, "local-agent.sqlite3"), {
        readOnly: true,
      });
      try {
        assert.equal(
          core.prepare("PRAGMA integrity_check").get().integrity_check,
          "ok",
        );
      } finally {
        core.close();
      }
      if (!result.originalTargetPreserved)
        result.error = `${result.error ?? ""}\n${name}: optional target changed`;
      // Explicitly repair this stopped owned copy. Product startup must not do this.
      fs.rmSync(target, { recursive: true, force: true });
      fs.copyFileSync(path.join(baseline, store.file), target);
      fs.chmodSync(target, 0o600);
      try {
        await boot(data);
        assert.equal(
          (await api("/api/domain/directory")).externalProblem,
          null,
        );
        assert.deepEqual((await api("/api/plugins")).items, legacy.items);
        assert.equal((await api("/api/sessions?agent=claude")).total, 1);
        result.explicitRepairRecovers = true;
      } finally {
        await stop();
      }
      if (result.error) report.failures.push({ name, error: result.error });
      report.cases.push(result);
      save();
      console.log(JSON.stringify(result));
    }
  assert.deepEqual(report.failures, []);
  assert.equal(active, 0);
  report.sourceFilesUnchanged = hash(native) === nativeHash;
  report.complete = true;
} catch (error) {
  report.error = error.stack;
  process.exitCode = 1;
} finally {
  await stop();
  report.unfinishedRequests = active;
  report.processesExited =
    !child || child.exitCode !== null || child.signalCode !== null;
  if (report.processesExited) {
    fs.rmSync(root, { recursive: true, force: true });
    report.temporaryDataRemoved = true;
  }
  save();
  console.log(
    JSON.stringify({
      complete: report.complete,
      failures: report.failures.length,
      error: report.error,
    }),
  );
}
