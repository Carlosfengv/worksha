import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  mkdtempSync,
  realpathSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";

const directory = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-accounts-")),
);
const home = path.join(directory, "home"),
  data = path.join(directory, "data");
mkdirSync(path.join(home, ".claude"), { recursive: true });
mkdirSync(path.join(home, ".codex"), { recursive: true });
const original =
  '{"env":{"ANTHROPIC_BASE_URL":"https://not-a-billing-origin.invalid","ANTHROPIC_AUTH_TOKEN":"native-secret-sentinel"},"unrelated":true}';
writeFileSync(path.join(home, ".claude/settings.json"), original);
writeFileSync(path.join(home, ".codex/config.toml"), "model='fixture'\n");
const token = randomUUID(),
  binary = path.resolve(
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? ".work/provider-tests-target/debug/local-agent-service",
  );
let child,
  stderr = "";
try {
  child = spawn(binary, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.on("data", (b) => (stderr += b));
  const ready = await Promise.race([
    once(createInterface({ input: child.stdout }), "line").then(([line]) =>
      JSON.parse(line),
    ),
    once(child, "exit").then(() => {
      throw Error("Service exited before ready");
    }),
    new Promise((_, reject) => {
      setTimeout(
        () => reject(Error("Service startup timed out")),
        10000,
      ).unref();
    }),
  ]);
  const url = `http://127.0.0.1:${ready.port}`;
  const request = async (route, method = "GET", body, extra = {}) =>
    fetch(url + route, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        ...(body ? { "Content-Type": "application/json" } : {}),
        ...extra,
      },
      body: body ? JSON.stringify(body) : undefined,
    });
  assert.equal((await fetch(url + "/api/accounts")).status, 401);
  assert.equal(
    (
      await request("/api/accounts", "GET", undefined, {
        Origin: "https://foreign.invalid",
      })
    ).status,
    401,
  );
  const list = await (await request("/api/accounts")).json();
  assert.equal(list.accounts.length, 3);
  assert.ok(list.accounts.every((a) => a.snapshot === null));
  assert.equal(
    list.accounts.find((a) => a.id === "native-api-claude").readable,
    false,
  );
  assert.ok(!JSON.stringify(list).includes("native-secret-sentinel"));
  const codex = list.accounts.find((a) => a.id === "native-codex");
  assert.equal(
    (
      await request("/api/accounts/native-codex/refresh", "POST", {
        expectedRevision: "wrong",
      })
    ).status,
    400,
  );
  const refused = await request("/api/accounts/native-codex/refresh", "POST", {
    expectedRevision: codex.sourceRevision,
  });
  assert.equal(refused.status, 400);
  assert.match((await refused.json()).message, /隔离来源/);
  const failure = await (await request("/api/accounts")).json();
  assert.equal(
    failure.accounts.find((a) => a.id === "native-codex").state,
    "failed",
  );
  const collector = path.join(data, "billing");
  const input = {
    apiKey: "not-persisted-sentinel",
    transcript_path: "not-persisted-sentinel",
    rate_limits: {
      five_hour: { used_percentage: 25, resets_at: 2000000000 },
      seven_day: { used_percentage: 60, resets_at: 2000500000 },
    },
  };
  const capture = spawnSync(binary, ["--claude-quota-stdin", collector], {
    input: JSON.stringify(input),
    encoding: "utf8",
  });
  assert.equal(capture.status, 0, capture.stderr);
  const updated = await (await request("/api/accounts")).json();
  const claude = updated.accounts.find((a) => a.id === "native-claude");
  assert.equal(claude.snapshot.windows[0].remainingPercent, 75);
  assert.equal(claude.snapshot.windows[1].remainingPercent, 40);
  assert.equal(claude.snapshot.resetCredits.availableCount, null);
  assert.equal(claude.snapshot.plan, null);
  const persisted = readFileSync(
    path.join(collector, "claude-quota.json"),
    "utf8",
  );
  assert.ok(!persisted.includes("sentinel"));
  assert.equal(
    statSync(path.join(collector, "claude-quota.json")).mode & 0o777,
    0o600,
  );
  assert.equal(
    readFileSync(path.join(home, ".claude/settings.json"), "utf8"),
    original,
  );
  assert.deepEqual(readdirSync(collector), ["claude-quota.json"]);
  assert.ok(!stderr.includes("sentinel"));
  console.log(
    "通过：账户接口鉴权 / Origin、目录不触发查询、读取版本复核、隔离来源拒读真实登录、共享失败状态、Claude CLI 采集与双窗口、未知 Plan / 券、白名单快照与原生配置保持不变。",
  );
} finally {
  if (child && child.exitCode === null) {
    child.kill();
    await once(child, "exit");
  }
  rmSync(directory, { recursive: true, force: true });
}
