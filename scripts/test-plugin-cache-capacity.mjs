// Real service admission over owned cache residue. No user sources or credentials.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { createInterface } from "node:readline";
import { once } from "node:events";
import { randomUUID, createHash } from "node:crypto";

const binary = path.resolve(
  process.env.PLUGIN_CACHE_BINARY ??
    process.env.PLUGIN_TEST_BINARY ??
    "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-cache-capacity-")),
);
const home = path.join(root, "home");
fs.mkdirSync(home);
const hash = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const report = {
  complete: false,
  binarySha256: hash(binary),
  scriptSha256: hash(new URL(import.meta.url)),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child, base, token, data;
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], {
    timeout: 30000,
    stdio: ["ignore", "pipe", "pipe"],
    encoding: "utf8",
  });
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const closed = once(child, "exit");
  child.kill("SIGTERM");
  let timer;
  try {
    await Promise.race([
      closed,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          child.kill("SIGKILL");
          reject(Error("owned service did not stop"));
        }, 8000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
async function boot(label) {
  assert(!child || child.exitCode !== null || child.signalCode !== null);
  data = path.join(root, label);
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
  let stderr = "";
  child.stderr.on("data", (b) => {
    stderr = (stderr + b).slice(-3000);
  });
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(20000),
    }),
    once(child, "exit").then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  assert.deepEqual((await api("/api/plugin-sources")).items, []); // initialize optional recovery first
  assert.deepEqual((await api("/api/plugins")).items, []);
}
async function api(route, body, expected = 200) {
  const response = await fetch(base + route, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20000),
  });
  const value = await response.json();
  assert.equal(response.status, expected, `${route}: ${JSON.stringify(value)}`);
  return value;
}
const prefix = (kind) =>
  kind === "history" ? "/api/plugin-sources" : "/api/plugins";
const install = (kind, review, expected = 200) =>
  api(
    prefix(kind) + "/install",
    {
      packageRef: review.packageRef,
      digest: review.digest,
      approvePublisher: true,
      ...(kind === "history" ? { expectedSigningKey: review.signingKey } : {}),
    },
    expected,
  );
function sparse(file, size) {
  assert(file.startsWith(root + path.sep));
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const fd = fs.openSync(file, "wx", 0o600);
  try {
    fs.ftruncateSync(fd, size);
  } finally {
    fs.closeSync(fd);
  }
}
function noWorkers() {
  const rows = execFileSync("ps", ["-axo", "ppid=,command="], {
    encoding: "utf8",
  }).split("\n");
  assert(
    !rows.some(
      (line) =>
        line.trim().startsWith(String(child.pid) + " ") &&
        line.includes(" --plugin-worker "),
    ),
  );
}
const publisher = path.join(root, "publisher.key");
const packages = new Map();
const caches = ["plugins", "plugin-sources"];
try {
  sdk("keygen", publisher);
  for (const [kind, example] of [
    ["session", "aurora-agent"],
    ["config", "json-config"],
    ["history", "full-history"],
  ]) {
    const author = path.join(root, `author-${kind}`);
    fs.cpSync(`sdk/plugins/examples/${example}/package`, author, {
      recursive: true,
    });
    const file = path.join(root, `${kind}.laplug`);
    sdk("pack", author, file, publisher);
    packages.set(kind, { file, hash: hash(file) });
  }
  for (const [kind, pkg] of packages) {
    for (const cache of caches) {
      await boot(`archive-${kind}-${cache}`);
      const review = await api(prefix(kind) + "/inspect", {
        path: pkg.file,
      });
      const snapshot = await api("/api/snapshot");
      const orphan = path.join(
        data,
        cache,
        "packages",
        "org.example.residue",
        "1.0.0",
        "f".repeat(64),
        "package.laplug",
      );
      sparse(orphan, 256 * 1048576);
      assert.equal((await install(kind, review, 400)).code, "resource_limit");
      assert.equal(fs.statSync(orphan).size, 256 * 1048576);
      assert.deepEqual((await api("/api/plugins")).items, []);
      assert.deepEqual((await api("/api/plugin-sources")).items, []);
      assert.equal((await api("/api/sessions")).total, 0);
      assert.deepEqual(
        (await api("/api/snapshot")).providers,
        snapshot.providers,
      );
      noWorkers();
      fs.unlinkSync(orphan); // our synthetic residue only, never an input package
      const installed = await install(kind, review);
      assert.equal(installed.enabled, false);
      if (kind === "history") assert.equal(installed.runtimeActive, false);
      assert.equal(hash(pkg.file), pkg.hash);
      report.cases.push(
        `${kind} install charges live archive residue in ${cache}, preserves native/provider reads and retries the same review`,
      );
      await stop();
    }
  }
  for (const [kind, pkg] of packages) {
    for (const cache of caches) {
      await boot(`staging-${kind}-${cache}`);
      const id = randomUUID();
      const sourceCache = cache === "plugin-sources";
      const orphan = sourceCache
        ? path.join(data, cache, "staging", `${id}.laplug`)
        : path.join(data, cache, "staging", id, "package.laplug");
      const bytes = (sourceCache ? 128 : 256) * 1048576 + 1;
      sparse(orphan, bytes);
      const route = prefix(kind) + "/inspect",
        body = { path: pkg.file };
      assert.equal((await api(route, body, 400)).code, "resource_limit");
      assert.equal((await api("/api/sessions")).total, 0);
      noWorkers();
      assert.equal(fs.statSync(orphan).size, bytes);
      fs.unlinkSync(orphan);
      if (!sourceCache) fs.rmdirSync(path.dirname(orphan));
      await api(route, body);
      assert.equal(hash(pkg.file), pkg.hash);
      report.cases.push(
        `${kind} review refuses over-budget staging in ${cache} without deleting it; retry succeeds after owned residue removal`,
      );
      await stop();
    }
  }
  await boot("case-aliases");
  const caseReview = await api("/api/plugin-sources/inspect", {
    path: packages.get("history").file,
  });
  const hidden = path.join(
    data,
    "plugin-sources/packages/ORG.EXAMPLE.RESIDUE/1.0.0",
    "F".repeat(64),
    "PACKAGE.LAPLUG",
  );
  sparse(hidden, 256 * 1048576);
  assert.equal(
    (await install("history", caseReview, 400)).code,
    "resource_limit",
  );
  fs.unlinkSync(hidden);
  const caseInstalled = await install("history", caseReview);
  const canonical = path.join(
    data,
    "plugin-sources/packages",
    caseInstalled.manifest.id,
    caseInstalled.manifest.version,
    caseInstalled.digest,
    "package.laplug",
  );
  const upper = path.join(path.dirname(canonical), "PACKAGE.LAPLUG"),
    originalCaseHash = hash(canonical),
    archiveBytes = fs.statSync(canonical).size;
  fs.renameSync(canonical, upper);
  const caseInsensitive = fs.existsSync(canonical);
  const floor = path.join(
    data,
    "plugins/packages/org.example.floor/1.0.0",
    "e".repeat(64),
    "package.laplug",
  );
  sparse(floor, 256 * 1048576 - archiveBytes);
  if (caseInsensitive) await install("history", caseReview);
  else
    assert.equal(
      (await install("history", caseReview, 400)).code,
      "resource_limit",
    );
  assert.equal(hash(upper), originalCaseHash);
  fs.renameSync(upper, canonical);
  fs.unlinkSync(floor);
  await install("history", caseReview);
  noWorkers();
  report.caseInsensitiveLookupObserved = caseInsensitive;
  report.cases.push(
    "case aliases remain charged; an existing canonical target is recognized by file identity at the exact cache cap without double charging; case-sensitive missing targets require a fresh allocation",
  );
  await stop();
  await boot("failed-removal");
  const review = await api("/api/plugin-sources/inspect", {
    path: packages.get("history").file,
  });
  const old = await install("history", review);
  const author = path.join(root, "author-history"),
    manifestFile = path.join(author, "manifest.json"),
    manifest = JSON.parse(fs.readFileSync(manifestFile));
  manifest.version = "1.1.0";
  fs.writeFileSync(manifestFile, JSON.stringify(manifest));
  const upgrade = path.join(root, "upgrade.laplug");
  sdk("pack", author, upgrade, publisher);
  const next = await api("/api/plugin-sources/inspect", { path: upgrade });
  const archive = path.join(
    data,
    "plugin-sources/packages",
    old.manifest.id,
    old.manifest.version,
    old.digest,
    "package.laplug",
  );
  const alias = path.join(root, "owned-original-alias.laplug");
  fs.linkSync(archive, alias);
  const originalHash = hash(alias);
  assert.equal(
    (
      await api(
        "/api/plugin-sources/state",
        {
          package: { pluginId: old.manifest.id, digest: old.digest },
          action: "remove",
        },
        403,
      )
    ).code,
    "permission_denied",
  );
  assert.deepEqual((await api("/api/plugin-sources")).items, []);
  assert.equal((await install("history", next, 403)).code, "permission_denied");
  assert.equal(hash(alias), originalHash);
  assert.equal(hash(archive), originalHash);
  noWorkers();
  fs.unlinkSync(alias);
  await install("history", next);
  report.cases.push(
    "actual remove commits disabled/removed state then fails safe on an owned hard link; the residual cannot bypass new admission, original bytes remain intact and the same reviewed upgrade retries",
  );
  await stop();
  report.complete = true;
} finally {
  await stop();
  fs.rmSync(root, { recursive: true, force: true });
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.PLUGIN_CACHE_REPORT)
    fs.writeFileSync(
      process.env.PLUGIN_CACHE_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
