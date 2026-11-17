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
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
      ".work/provider-tests-target/debug/local-agent-service",
  );
let child,
  stderr = "";
try {
  child = spawn(binary, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
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
  assert.equal(codex.codexRead.authorized, false);
  assert.equal(
    (
      await request("/api/accounts/native-codex/refresh", "POST", {
        expectedRevision: "wrong",
      })
    ).status,
    400,
  );
  const unapproved = await request(
    "/api/accounts/native-codex/refresh",
    "POST",
    {
      expectedRevision: codex.sourceRevision,
    },
  );
  assert.equal(unapproved.status, 400);
  assert.match((await unapproved.json()).message, /先确认手动读取/);
  const grant = await request("/api/account-collectors/codex/consent", "PUT", {
    expectedRevision: codex.sourceRevision,
    enabled: true,
  });
  assert.equal(grant.status, 200);
  assert.deepEqual(await grant.json(), { authorized: true });
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
  const revoke = await request("/api/account-collectors/codex/consent", "PUT", {
    expectedRevision: codex.sourceRevision,
    enabled: false,
  });
  assert.equal(revoke.status, 200);
  const revoked = await request("/api/accounts/native-codex/refresh", "POST", {
    expectedRevision: codex.sourceRevision,
  });
  assert.equal(revoked.status, 400);
  assert.match((await revoked.json()).message, /先确认手动读取/);
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
  assert.deepEqual(readdirSync(collector).sort(), [
    "claude-quota.json",
    "codex-read-consent.json",
  ]);
  const statusLine = async (action) => {
    const result = await request(
      "/api/account-collectors/claude/statusline",
      "POST",
      action,
    );
    const body = await result.json();
    assert.equal(result.status, 200, JSON.stringify(body));
    return body;
  };
  const configPath = path.join(home, ".claude/settings.json");
  let line = await statusLine({ kind: "status" });
  assert.equal(line.config.state, "disabled");
  const preview = await statusLine({
    kind: "preview",
    expectedRevision: line.config.revision,
    enabled: true,
  });
  assert.equal(readFileSync(configPath, "utf8"), original);
  line = await statusLine({ kind: "apply", reviewId: preview.reviewId });
  assert.equal(line.config.state, "installed");
  assert.equal(
    line.received,
    false,
    "standalone history does not verify the installed entry",
  );
  assert.deepEqual(
    JSON.parse(readFileSync(configPath, "utf8")).statusLine,
    preview.statusLine,
  );
  const helper = readdirSync(collector).find((name) =>
    name.startsWith("usage-reader-"),
  );
  const generation = preview.statusLine.command.split(" ").at(-1);
  const callback = () =>
    spawnSync(
      path.join(collector, helper),
      ["--claude-quota-stdin", collector, "--managed", configPath, generation],
      { input: JSON.stringify(input), encoding: "utf8" },
    );
  const managedCapture = callback();
  assert.equal(managedCapture.status, 0, managedCapture.stderr);
  line = await statusLine({ kind: "status" });
  assert.equal(line.received, true);
  const managedFile = path.join(collector, "claude-quota-managed.json");
  const managedHistory = readFileSync(managedFile, "utf8");
  assert.ok(!managedHistory.includes("sentinel"));
  const disable = await statusLine({
    kind: "preview",
    expectedRevision: line.config.revision,
    enabled: false,
  });
  line = await statusLine({ kind: "apply", reviewId: disable.reviewId });
  assert.equal(line.config.state, "disabled");
  assert.equal(line.received, false);
  assert.deepEqual(
    JSON.parse(readFileSync(configPath, "utf8")),
    JSON.parse(original),
  );
  const retiredCallback = callback();
  assert.equal(retiredCallback.status, 0, retiredCallback.stderr);
  assert.equal(readFileSync(managedFile, "utf8"), managedHistory);
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
