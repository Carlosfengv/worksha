import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
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
const root = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-read-admission-")),
);
const home = path.join(root, "home");
const data = path.join(root, "data");
mkdirSync(home);
mkdirSync(path.join(data, "gateway-lan"), { recursive: true, mode: 0o700 });
const token = randomUUID(),
  clientId = randomUUID();
// A synthetic, unconfigured LAN client exercises registry-only revocation while
// the unrelated configuration database is locked. No model listener is enabled.
writeFileSync(
  path.join(data, "gateway-lan/clients.json"),
  JSON.stringify([
    {
      id: clientId,
      name: "Read admission control",
      agent: "claude",
      providerId: "fixture-provider",
      models: ["fixture-model"],
      concurrency: 1,
      enabled: true,
      createdAt: new Date().toISOString(),
      keyHash: createHash("sha256").update(randomUUID()).digest("hex"),
    },
  ]),
  { mode: 0o600 },
);
const routes = [
  "/api/accounts",
  "/api/sessions",
  "/api/calls",
  "/api/agents/claude/memory",
  "/api/mcp",
  "/api/sessions/missing",
  "/api/projects",
  "/api/projects/missing",
  "/api/projects/missing/diff/file",
  "/api/requests",
  "/api/overview?range=24&metric=calls&dimension=agent&source=gateway",
  "/api/request-dashboard",
  "/api/request-detail?id=missing",
  "/api/agent-usage?agent=claude&source=gateway",
  "/api/provider-usage?source=gateway",
  "/api/associated-costs?scope=session&id=missing",
];
const report = {
  complete: false,
  binarySha256: createHash("sha256").update(readFileSync(binary)).digest("hex"),
  tokioWorkers: 1,
  queryCapacity: 8,
  managementCapacity: 32,
  routes: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
const pending = [],
  aborters = [];
const activeReads = new Map();
let child,
  db,
  base,
  releaseTimer,
  locked = false,
  releasedByTimer = false;
let writerFinished = false;
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function release() {
  if (locked) {
    db.exec("ROLLBACK");
    locked = false;
  }
}
async function api(
  route,
  body,
  method = body === undefined ? "GET" : "POST",
  signal,
) {
  const response = await fetch(base + route, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: signal ?? AbortSignal.timeout(6000),
  });
  return { status: response.status, body: await response.json() };
}
try {
  child = spawn(binary, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
      TOKIO_WORKER_THREADS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.resume();
  const lines = createInterface({ input: child.stdout });
  let readyTimer;
  const ready = await Promise.race([
    once(lines, "line").then(([line]) => JSON.parse(line)),
    once(child, "exit").then(() => {
      throw Error("isolated service exited before readiness");
    }),
    new Promise((_, reject) => {
      readyTimer = setTimeout(
        () => reject(Error("readiness timed out")),
        15000,
      );
    }),
  ]).finally(() => clearTimeout(readyTimer));
  base = `http://127.0.0.1:${ready.port}`;
  const snapshot = await api("/api/snapshot");
  assert.equal(snapshot.status, 200);
  routes.push(
    `/api/settings/retention-preview?storeId=${snapshot.body.settingsWriteProtocol.storeId}&expectedSettingsRevision=${snapshot.body.settingsRevision}&days=30`,
    `/api/settings/retention-inventory?storeId=${snapshot.body.settingsWriteProtocol.storeId}&expectedSettingsRevision=${snapshot.body.settingsRevision}&days=30`,
    `/api/settings/retention-inventory?storeId=${snapshot.body.settingsWriteProtocol.storeId}&expectedSettingsRevision=${snapshot.body.settingsRevision}&days=30&inventoryVersion=2`,
  );
  const provider = {
    ...snapshot.body.providers[0],
    name: "Read admission fixture",
  };
  const baseline = [];
  for (const route of routes) {
    const value = await api(route);
    assert.notEqual(value.body.code, "busy", route);
    baseline.push({
      status: value.status,
      keys: Object.keys(value.body).sort(),
    });
  }
  db = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  db.exec("BEGIN IMMEDIATE");
  locked = true;
  releaseTimer = setTimeout(() => {
    releasedByTimer = true;
    release();
  }, 3000);
  const writer = api("/api/providers", provider, "PUT").then((value) => {
    writerFinished = true;
    return value;
  });
  pending.push(writer);
  assert(!writerFinished, "fixture write must be waiting on real SQLite lock");
  let submittedReads = 0;
  function submitRead() {
    const controller = new AbortController();
    aborters.push(controller);
    const request = api(
      "/api/provider-usage?source=gateway",
      undefined,
      "GET",
      controller.signal,
    )
      .catch((error) => {
        if (controller.signal.aborted) return { aborted: true };
        throw error;
      })
      .finally(() => activeReads.delete(controller));
    activeReads.set(controller, request);
    pending.push(request);
    submittedReads++;
  }
  const fillDeadline = performance.now() + 700;
  let saturated = false;
  while (performance.now() < fillDeadline) {
    // Wait for actual ownership: early reads may finish before the writer has
    // reached its SQL lock. Never create more than eight in-flight data reads.
    if (activeReads.size < 8 && submittedReads < 32) submitRead();
    await pause(2);
    const probe = await api("/api/calls");
    if (probe.body.code === "busy") {
      assert.equal(probe.status, 503);
      saturated = true;
      break;
    }
    await pause(5);
  }
  assert(
    saturated,
    "eight blocked data reads must exhaust their own admission",
  );
  assert.equal(
    activeReads.size,
    8,
    "exactly eight real reads must still be pending",
  );
  const blockedReads = [...activeReads.values()];
  report.submittedDataReads = submittedReads;
  report.completedReadsBeforeSaturation = submittedReads - activeReads.size;
  activeReads.keys().next().value.abort();
  const started = performance.now();
  const [rejected, health, plugins, revoked] = await Promise.all([
    Promise.all(
      routes.map(async (route) => {
        const value = await api(
          route,
          undefined,
          "GET",
          AbortSignal.timeout(1000),
        );
        assert.equal(value.status, 503, route);
        assert.equal(value.body.code, "busy", route);
        return { route, status: value.status, code: value.body.code };
      }),
    ),
    api("/api/runtime", undefined, "GET", AbortSignal.timeout(1000)),
    api("/api/plugins", undefined, "GET", AbortSignal.timeout(1000)),
    api(
      `/api/gateway-lan/clients/${clientId}/revoke`,
      {},
      "POST",
      AbortSignal.timeout(1000),
    ),
  ]);
  assert.equal(health.status, 200);
  assert.equal(plugins.status, 200);
  assert.equal(revoked.status, 200);
  assert.equal(revoked.body.revoked, true);
  const registry = JSON.parse(
    readFileSync(path.join(data, "gateway-lan/clients.json"), "utf8"),
  );
  assert.equal(
    (Array.isArray(registry) ? registry : registry.clients).find(
      (c) => c.id === clientId,
    ).enabled,
    false,
  );
  assert(
    locked && !releasedByTimer && !writerFinished,
    "lock must still be held during independent management checks",
  );
  report.overloadResponses = rejected;
  report.controlObservationMs = performance.now() - started;
  report.managementRegistryOnlyRevocationDuringReadPressure = true;
  report.noClaimThatSqlDependentManagementAvoidsTheSameDatabaseLock = true;
  const refreshAdmission = await api(
    "/api/accounts/native-codex/refresh",
    { expectedRevision: "" },
    "POST",
    AbortSignal.timeout(1000),
  );
  assert.equal(refreshAdmission.status, 503);
  assert.equal(refreshAdmission.body.code, "busy");
  assert.equal(refreshAdmission.body.outcomeUnknown, false);
  report.accountRefreshAdmissionBeforeQuery = {
    status: refreshAdmission.status,
    code: refreshAdmission.body.code,
    outcomeUnknown: refreshAdmission.body.outcomeUnknown,
  };
  const unauth = await fetch(base + "/api/projects");
  assert.equal(unauth.status, 401);
  const origin = await fetch(base + "/api/calls", {
    headers: { Authorization: `Bearer ${token}`, Origin: base },
  });
  assert.equal(origin.status, 401);
  assert(locked && !releasedByTimer);
  clearTimeout(releaseTimer);
  release();
  const settled = await Promise.all(pending);
  assert.equal(settled[0].status, 200);
  assert.equal(settled.filter((v) => v.aborted).length, 1);
  const blockedResults = await Promise.all(blockedReads);
  assert.equal(blockedResults.filter((v) => v.aborted).length, 1);
  assert(
    blockedResults.filter((v) => !v.aborted).every((v) => v.status === 200),
  );
  assert(
    settled
      .slice(1)
      .every(
        (v) =>
          v.aborted ||
          v.status === 200 ||
          (v.status === 503 && v.body.code === "busy"),
      ),
  );
  for (let i = 0; i < routes.length; i++) {
    const value = await api(routes[i]);
    assert.equal(value.status, baseline[i].status, routes[i]);
    assert.deepEqual(
      Object.keys(value.body).sort(),
      baseline[i].keys,
      routes[i],
    );
    assert.notEqual(value.body.code, "busy", routes[i]);
    report.routes.push({
      route: routes[i],
      statusBeforeAndAfter: value.status,
    });
  }
  const final = await api("/api/snapshot");
  assert.equal(
    final.body.providers.find((p) => p.id === provider.id).revision,
    provider.revision + 1,
  );
  report.blockedDataRequests = 8;
  report.disconnectedReadClients = 1;
  report.authAndOriginBeforeAdmission = true;
  report.recoveryAfterExplicitLockRelease = true;
  report.complete = true;
} finally {
  clearTimeout(releaseTimer);
  release();
  aborters.forEach((controller) => controller.abort());
  await Promise.allSettled(pending);
  db?.close();
  if (child && child.exitCode === null && child.signalCode === null) {
    const done = once(child, "exit");
    child.kill("SIGINT");
    const killTimer = setTimeout(() => child.kill("SIGKILL"), 5000);
    await done.finally(() => clearTimeout(killTimer));
  }
  rmSync(root, { recursive: true, force: true });
  report.ownedServiceStopped = true;
  report.temporaryDataRemoved = true;
  if (process.env.READ_ADMISSION_REPORT) {
    const out = path.resolve(process.env.READ_ADMISSION_REPORT);
    mkdirSync(path.dirname(out), { recursive: true });
    writeFileSync(out, JSON.stringify(report, null, 2) + "\n");
  }
}
console.log(JSON.stringify(report, null, 2));
