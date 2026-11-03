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
const binarySha256 = createHash("sha256")
  .update(readFileSync(binary))
  .digest("hex");
const root = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-admission-")),
);
const home = path.join(root, "source");
mkdirSync(home);
const data = path.join(root, "data");
const token = randomUUID();
const report = {
  complete: false,
  binarySha256,
  tokioWorkers: 1,
  admissionCapacity: 32,
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child,
  db,
  releaseTimer,
  base,
  locked = false,
  releasedByTimer = false;
const pending = [];
const diagnostics = {
  writerBeforeRelease: null,
  readsBeforeRelease: 0,
  probes: [],
};
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const release = () => {
  if (locked) {
    db.exec("ROLLBACK");
    locked = false;
  }
};
async function api(
  route,
  body,
  method = body === undefined ? "GET" : "POST",
  budget = 6000,
) {
  const response = await fetch(base + route, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(budget),
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
  const provider = { ...snapshot.body.providers[0], name: "Admission fixture" };
  db = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  db.exec("BEGIN IMMEDIATE");
  locked = true;
  releaseTimer = setTimeout(() => {
    releasedByTimer = true;
    release();
  }, 2500);
  const write = api("/api/providers", provider, "PUT").then((result) => {
    if (locked)
      diagnostics.writerBeforeRelease = {
        status: result.status,
        code: result.body.code,
        message: result.body.message,
      };
    return result;
  });
  pending.push(write);
  let saturated = false;
  let submittedReads = 0;
  const filling = performance.now();
  while (performance.now() - filling < 700 && submittedReads < 96) {
    assert.equal(
      diagnostics.writerBeforeRelease,
      null,
      "fixture writer finished before saturation",
    );
    pending.push(
      api("/api/model-services").then((result) => {
        if (locked) diagnostics.readsBeforeRelease++;
        return result;
      }),
    );
    submittedReads++;
    await pause(2);
    const probe = await api("/api/plugins", undefined, "GET", 1000);
    if (diagnostics.probes.length < 10)
      diagnostics.probes.push({ status: probe.status, code: probe.body.code });
    if (probe.body.code === "busy") {
      assert.equal(probe.status, 503);
      saturated = true;
      break;
    }
    await pause(5);
  }
  assert(
    saturated,
    "shared admission was not saturated by the blocked fixture work",
  );
  const query = { pluginId: "org.example.fixture", digest: "a".repeat(64) };
  const requests = [
    ["/api/gateway-lan", undefined, "GET", 409],
    [
      "/api/gateway-lan/operations/" +
        randomUUID() +
        "?storeId=" +
        snapshot.body.settingsWriteProtocol.storeId,
      undefined,
      "GET",
      409,
    ],
    [
      "/api/gateway-lan/operations",
      {
        storeId: snapshot.body.settingsWriteProtocol.storeId,
        operationId: randomUUID(),
        action: { kind: "revoke", clientId: randomUUID() },
      },
      "POST",
      409,
    ],
    ["/api/plugins", undefined, "GET", 503],
    ["/api/plugins/state", { ...query, action: "disable" }, "POST", 503],
    [
      "/api/plugin-extensions/status",
      { pluginId: "", digest: "" },
      "POST",
      503,
    ],
    [
      "/api/plugin-extensions/action",
      {
        ...query,
        action: {
          kind: "bind-protocol",
          providerId: provider.id,
          allowPrivateNetwork: false,
        },
      },
      "POST",
      503,
    ],
    [
      "/api/plugin-extensions/action",
      { ...query, action: { kind: "prepare-config", providerId: provider.id } },
      "POST",
      503,
    ],
    ["/api/model-services", undefined, "GET", 409],
    ["/api/provider-imports", undefined, "GET", 409],
    ["/api/providers", provider, "PUT", 409],
    ["/api/session-sources", { agentId: "claude", enabled: false }, "PUT", 503],
    [
      "/api/project-visibility",
      { projectId: "fixture", hidden: true },
      "PUT",
      503,
    ],
    [
      "/api/observation/bindings",
      {
        agentId: "claude",
        providerId: provider.id,
        expectedRevision: 0,
        enabled: false,
      },
      "PUT",
      503,
    ],
    [
      "/api/dynamic-connections",
      {
        agentId: "claude",
        expectedRevision: "fixture",
        providerId: provider.id,
      },
      "POST",
      409,
    ],
    [
      "/api/dynamic-routes",
      {
        operationId: randomUUID(),
        bindingId: randomUUID(),
        expectedGeneration: "fixture",
        providerId: provider.id,
        expectedProviderRevision: provider.revision,
        model: "fixture",
      },
      "POST",
      409,
    ],
    ["/api/dynamic-routes/" + randomUUID(), { action: "cancel" }, "POST", 409],
    ["/api/account-collectors/claude", {}, "POST", 503],
    ["/api/snapshot", undefined, "GET", 503],
    ["/api/settings/operations/" + randomUUID(), undefined, "GET", 503],
    [
      "/api/settings/scoped/operations/" +
        randomUUID() +
        "?storeId=" +
        snapshot.body.settingsWriteProtocol.storeId,
      undefined,
      "GET",
      503,
    ],
    [
      "/api/settings",
      {
        storeId: snapshot.body.settingsWriteProtocol.storeId,
        value: { captureContent: false, retentionDays: 30 },
        expectedRevision: snapshot.body.settingsRevision,
        operationId: randomUUID(),
      },
      "PUT",
      503,
    ],
  ];
  const started = performance.now();
  const [health, rejected, refreshes] = await Promise.all([
    api("/api/runtime", undefined, "GET", 1000).then((value) => ({
      ...value,
      elapsedMs: performance.now() - started,
    })),
    Promise.all(
      requests.map(async ([route, body, method, expected]) => {
        const result = await api(route, body, method, 1000);
        assert.equal(result.status, expected, route);
        assert.equal(result.body.code, "busy", route);
        if (
          /^\/api\/(session-sources|project-visibility|observation\/bindings|dynamic-|account-collectors)/.test(
            route,
          )
        ) {
          assert.equal(result.body.outcomeUnknown, false, route);
        }
        return { route, status: result.status, code: result.body.code };
      }),
    ),
    Promise.all(
      Array.from({ length: 32 }, () =>
        api("/api/sessions/refresh", {}, "POST", 1000),
      ),
    ),
  ]);
  assert(
    refreshes.every(
      (value) => value.status === 200 && value.body.status === "scanning",
    ),
  );
  report.coalescedRefreshResponsesWhileManagementFull = refreshes.length;
  assert.equal(health.status, 200);
  assert(health.elapsedMs < 1000);
  assert(
    locked && !releasedByTimer,
    "fixture lock expired before the liveness observation",
  );
  report.healthDuringLockMs = health.elapsedMs;
  report.overloadResponses = rejected;
  report.cases.push(
    "shared admission rejects excess plugin, provider and model-service work while runtime identity remains responsive under a real SQLite write lock",
  );
  clearTimeout(releaseTimer);
  release();
  const settled = await Promise.all(pending);
  assert.equal(settled[0].status, 200);
  assert(
    settled
      .slice(1)
      .every(
        (value) =>
          value.status === 200 ||
          (value.status === 409 && value.body.code === "busy"),
      ),
  );
  report.submittedReadCalls = submittedReads;
  report.readCallsRejectedAtAdmission = settled
    .slice(1)
    .filter((value) => value.body.code === "busy").length;
  const after = await api("/api/snapshot");
  assert.equal(after.status, 200);
  assert.equal(
    after.body.providers.find((p) => p.id === provider.id).revision,
    provider.revision + 1,
  );
  assert.equal((await api("/api/plugins")).status, 200);
  report.cases.push(
    "admitted work completes after explicit lock release and capacity becomes available again",
  );
  report.complete = true;
} finally {
  report.diagnostics = diagnostics;
  clearTimeout(releaseTimer);
  release();
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
  if (process.env.MANAGEMENT_ADMISSION_REPORT) {
    const out = path.resolve(process.env.MANAGEMENT_ADMISSION_REPORT);
    mkdirSync(path.dirname(out), { recursive: true });
    writeFileSync(out, JSON.stringify(report, null, 2) + "\n");
  }
}
console.log(JSON.stringify(report, null, 2));
