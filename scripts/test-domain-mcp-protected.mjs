// Real macOS protected-path worker coverage using only an owned workspace fixture.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { homedir } from "node:os";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
import { createAntigravityFixture } from "./fixtures/antigravity.mjs";

const binary = path.resolve(
  process.env.PROTECTED_MCP_BINARY ?? "target/debug/local-agent-service",
);
const baseline =
  process.env.PROTECTED_MCP_BASELINE_BINARY &&
  path.resolve(process.env.PROTECTED_MCP_BASELINE_BINARY);
const before = process.env.PROTECTED_MCP_EXPECT_REJECT === "1";
assert.equal(
  process.platform,
  "darwin",
  "This test requires the real macOS protected-path branch.",
);
fs.mkdirSync(path.resolve(".work"), { recursive: true });
const workspace = fs.realpathSync(path.resolve(".work"));
assert(
  workspace.startsWith(path.join(homedir(), "Downloads") + path.sep),
  "Use the managed workspace inside Downloads; never substitute real configuration files.",
);
const root = fs.mkdtempSync(path.join(workspace, "domain-mcp-protected-"));
const fixture = createAntigravityFixture(root);
const { home, project, db } = fixture;
const files = [
  path.join(home, ".gemini/antigravity/mcp_config.json"),
  path.join(home, ".gemini/config/mcp_config.json"),
  path.join(project, ".agents/mcp_config.json"),
];
const names = ["fixture-mcp", "secondary-mcp", "project-mcp"];
const marker = path.join(root, "must-not-run-config-command");
for (let n = 0; n < files.length; n++) {
  fs.mkdirSync(path.dirname(files[n]), { recursive: true });
  fs.writeFileSync(
    files[n],
    JSON.stringify({
      mcpServers: {
        [names[n]]: {
          command: "/usr/bin/touch",
          args: [marker],
          env: { TOKEN: "protected-fixture-secret" },
        },
      },
    }),
  );
}
const sha = (p) =>
  createHash("sha256").update(fs.readFileSync(p)).digest("hex");
const hashes = files.map(sha);
const rowsBefore = db.prepare("SELECT COUNT(*) AS n FROM steps").get().n;
const report = {
  complete: false,
  scope:
    "macOS protected Antigravity global/project configuration worker and common catalog",
  protectedPathVerified: true,
  binarySha256: sha(binary),
  realModelCalls: 0,
  realKeychainCalls: 0,
  cases: [],
};
let child, base, token;
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  child.kill();
  let timer;
  try {
    await Promise.race([
      exited,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          child.kill("SIGKILL");
          reject(Error("Owned service did not exit"));
        }, 5000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
async function boot(exe) {
  token = randomUUID();
  // Keep the existing test-vault isolation guard: this owned mkdtemp directory
  // is also this child process's temporary root, inside the protected workspace.
  child = spawn(exe, ["--data-dir", path.join(root, "data")], {
    env: {
      ...process.env,
      TMPDIR: root,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", (chunk) => {
    stderr = (stderr + chunk).slice(-2000);
  });
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(15000) }),
    once(child, "exit").then(() => {
      throw Error(
        `Owned service exited before readiness: ${stderr.replaceAll(token, "<token>").replaceAll("protected-fixture-secret", "<fixture-secret>")}`,
      );
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
}
async function api(route, body, method = body === undefined ? "GET" : "POST") {
  const r = await fetch(base + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(15000),
  });
  assert.equal(r.status, 200, route);
  return r.json();
}
function worker(exe, expectedSuccess) {
  for (const file of files) {
    const r = spawnSync(exe, ["--read-mcp-config", file], {
      encoding: "utf8",
      timeout: 10000,
    });
    assert.equal(
      r.status === 0,
      expectedSuccess,
      "Known Antigravity configuration filename must follow the expected worker policy.",
    );
    if (expectedSuccess)
      assert.deepEqual(
        JSON.parse(JSON.parse(r.stdout)[0]),
        JSON.parse(fs.readFileSync(file)),
      );
  }
  const rejected = spawnSync(
    exe,
    ["--read-mcp-config", path.join(root, "credentials.json")],
    { encoding: "utf8", timeout: 10000 },
  );
  assert.notEqual(rejected.status, 0, "Unrelated filenames remain rejected.");
}
async function observe(exe, supported) {
  worker(exe, supported);
  await boot(exe);
  try {
    await api(
      "/api/session-sources",
      { agentId: "antigravity", enabled: true },
      "PUT",
    );
    let sessions;
    for (let n = 0; n < 150; n++) {
      sessions = await api("/api/sessions?agent=antigravity");
      if (sessions.total === 1) break;
      await new Promise((r) => setTimeout(r, 40));
    }
    assert.equal(sessions.total, 1);
    const legacy = await api("/api/mcp?agent=antigravity");
    const configured = legacy.services
      .filter(
        (s) =>
          s.agentId === "antigravity" && s.configurationStatus === "present",
      )
      .map((s) => s.name)
      .sort();
    assert.deepEqual(configured, supported ? [...names].sort() : []);
    assert.equal(legacy.totalCalls, 1);
    assert(!JSON.stringify(legacy).includes("protected-fixture-secret"));
    if (supported) {
      const q = {
        scope: { kind: "builtin", sourceIds: ["antigravity"] },
        kind: "mcp-catalog",
        snapshotToken: null,
        cursor: null,
      };
      let catalog;
      for (let n = 0; n < 150; n++) {
        const r = await fetch(base + "/api/domain/facts", {
          method: "POST",
          headers: {
            authorization: `Bearer ${token}`,
            "content-type": "application/json",
          },
          body: JSON.stringify(q),
          signal: AbortSignal.timeout(15000),
        });
        if (r.status === 409) {
          await new Promise((r) => setTimeout(r, 40));
          continue;
        }
        assert.equal(r.status, 200);
        catalog = await r.json();
        if (
          catalog.items.length === 3 &&
          catalog.sourceStates[0].coverage === "complete"
        )
          break;
        await new Promise((r) => setTimeout(r, 40));
      }
      assert.equal(catalog.items.length, 3);
      assert.equal(catalog.sourceStates[0].coverage, "complete");
      assert.deepEqual(
        catalog.items.map((r) => r.value.value.serverName).sort(),
        [...names].sort(),
      );
      assert(catalog.items.every((r) => r.value.value.toolNames === null));
      assert(!JSON.stringify(catalog).includes("protected-fixture-secret"));
      await api(
        "/api/session-sources",
        { agentId: "antigravity", enabled: false },
        "PUT",
      );
      const r = await fetch(base + "/api/domain/facts", {
        method: "POST",
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "application/json",
        },
        body: JSON.stringify({ ...q, snapshotToken: catalog.snapshotToken }),
        signal: AbortSignal.timeout(15000),
      });
      assert.equal(r.status, 409);
    }
    return {
      sessionId: sessions.items[0].id,
      executionId: legacy.calls[0].call.id,
    };
  } finally {
    await stop();
  }
}
try {
  const old = baseline ? await observe(baseline, false) : null;
  const current = await observe(binary, !before);
  if (old)
    assert.deepEqual(
      current,
      old,
      "Configuration discovery must not change native history identities.",
    );
  assert.deepEqual(files.map(sha), hashes);
  assert(!fs.existsSync(marker));
  assert.equal(
    db.prepare("SELECT COUNT(*) AS n FROM steps").get().n,
    rowsBefore,
  );
  report.cases.push(
    before
      ? "baseline worker rejects all three standard Antigravity configuration paths"
      : "all three global/project paths traverse the real worker and common catalog, with unknown tools and revoke fencing",
    "unrelated filenames rejected; no configured command executed; native session/execution identities and source files preserved",
  );
  report.complete = true;
} finally {
  await stop();
  db.close();
  fs.rmSync(root, { recursive: true, force: true });
  report.ownedServiceStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.PROTECTED_MCP_REPORT)
    fs.writeFileSync(
      process.env.PROTECTED_MCP_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
