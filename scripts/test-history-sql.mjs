import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

// Isolated fixtures only. An optional previous service binary independently
// verifies the CSV encoder and query contract; regular CI needs no old artifact.
const root = realpathSync(mkdtempSync(path.join(tmpdir(), "sql-history-")));
const count = Number(process.env.LOCAL_AGENT_HISTORY_ROWS ?? 5005);
const modelCount = Number(process.env.LOCAL_AGENT_HISTORY_MODELS ?? 37);
const binary =
  process.env.LOCAL_AGENT_HISTORY_BINARY ?? "target/debug/local-agent-service";
const testVault = process.env.LOCAL_AGENT_HISTORY_PRODUCTION !== "1";
const token = randomUUID();
const services = [];
const databases = [];
const rows = Array.from({ length: count }, (_, i) => ({
  id: `history-${String(i).padStart(8, "0")}`,
  agentId: "claude",
  agentProductId: "claude-code-cli",
  provider: i % 2 ? "测试,连接" : "=CSV 公式边界",
  providerId: "archived",
  endpoint: "https://api.deepseek.com",
  bindingRevision: 1,
  model:
    modelCount === 1 ? "deepseek-flash" : `deepseek-flash-${i % modelCount}`,
  protocol: "messages",
  revision: 1,
  startedAt: new Date(Date.UTC(2026, 9, 19, 1, 0, i % 3600)).toISOString(),
  endedAt: "2026-10-19T02:00:01Z",
  status: "success",
  durationMs: 1000,
  ttftMs: 100,
  usage: {
    inputTokens: 10,
    outputTokens: 2,
    totalTokens: 12,
    cachedInputTokens: 0,
  },
}));
function seed(name) {
  const directory = path.join(root, name);
  const home = path.join(directory, "home");
  mkdirSync(home, { recursive: true });
  mkdirSync(path.join(directory, "observation"));
  const db = new DatabaseSync(
    path.join(directory, "observation/observations.sqlite3"),
  );
  databases.push(db);
  db.exec(
    "PRAGMA journal_mode=WAL;CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT NOT NULL);BEGIN",
  );
  const insert = db.prepare("INSERT INTO model_observations VALUES(?,?,?,?)");
  for (const row of rows)
    insert.run(row.id, row.startedAt, row.revision, JSON.stringify(row));
  db.exec("COMMIT");
  return { directory, home, db };
}
async function start(binary, fixture, testVault = true) {
  let stderr = "";
  const child = spawn(binary, ["--data-dir", fixture.directory], {
    env: {
      ...process.env,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_SOURCE_HOME: fixture.home,
      LOCAL_AGENT_TEST_MEMORY_VAULT: testVault ? "1" : undefined,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  services.push(child);
  child.stderr.on("data", (b) => (stderr += b));
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line"),
    once(child, "exit").then(() => {
      throw Error(stderr);
    }),
  ]);
  const base = `http://127.0.0.1:${JSON.parse(line).port}`;
  return async (route) => {
    const deadline = Date.now() + 60000;
    for (;;) {
      const response = await fetch(base + route, {
        headers: { authorization: `Bearer ${token}` },
      });
      const body = await response.text();
      assert.ok(body, `empty response ${response.status}: ${route}`);
      const result = JSON.parse(body);
      if (response.status === 200) return result;
      if (
        result.message?.includes("历史查询正在更新") &&
        Date.now() < deadline
      ) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        continue;
      }
      assert.equal(response.status, 200, JSON.stringify(result));
    }
  };
}
const query = {
  start: "2026-10-18T00:00:00Z",
  end: "2026-10-20T00:00:00Z",
  metric: "calls",
};
const route = (q) => "/api/request-dashboard?" + new URLSearchParams(q);
const rawDigest = (db) => {
  const hash = createHash("sha256");
  for (const row of db
    .prepare("SELECT value FROM model_observations ORDER BY id")
    .iterate())
    hash.update(row.value);
  return hash.digest("hex");
};
const providerRoute = "/api/provider-usage?source=gateway&providerId=_unknown";
const modelRoute = "/api/agent-usage?source=gateway&agent=claude";
async function reconcileUsage(
  api,
  provider,
  model,
  expectedCalls,
  expectedTokens,
) {
  assert.equal(provider.selected.calls, expectedCalls);
  assert.equal(
    provider.models.reduce((sum, item) => sum + item.totals.calls, 0),
    expectedCalls,
  );
  assert.equal(
    provider.models.reduce(
      (sum, item) => sum + item.totals.usage.totalTokens,
      0,
    ),
    expectedTokens,
  );
  const items = [];
  const summary = model.summary;
  const total = model.total;
  for (;;) {
    assert.deepEqual(model.summary, summary);
    assert.equal(model.total, total);
    items.push(...model.items);
    if (!model.nextCursor) break;
    model = await api(
      `${modelRoute}&cursor=${encodeURIComponent(model.nextCursor)}`,
    );
  }
  assert.equal(items.length, total);
  assert.equal(
    new Set(items.map((item) => JSON.stringify([item.providerId, item.model])))
      .size,
    total,
  );
  assert.equal(
    items.reduce((sum, item) => sum + item.totals.attempts, 0),
    expectedCalls,
  );
  assert.equal(
    items.reduce((sum, item) => sum + item.totals.usage.totalTokens, 0),
    expectedTokens,
  );
}
try {
  const fixture = seed("current");
  const before = rawDigest(fixture.db);
  let api = await start(binary, fixture, testVault);
  const first = await api(route(query));
  assert.equal(first.total, count);
  assert.equal(first.summary.attempts, count);
  assert.equal(first.summary.usage.totalTokens, count * 12);
  assert.equal(
    first.groups.reduce((n, g) => n + g.value, 0),
    count,
  );
  assert.equal(
    first.buckets.reduce((n, b) => n + b.value, 0),
    count,
  );
  assert.equal(
    first.heatmap.days.reduce((n, d) => n + d.records, 0),
    count,
  );
  const deepOffset = Math.floor((count - 1) / 40) * 40;
  const frozen = {
    ...query,
    snapshot: first.snapshot,
    cursor: `${first.snapshot}:${deepOffset}`,
  };
  const deep = await api(route(frozen));
  assert.equal(deep.items.length, count - deepOffset);
  assert.ok(deep.items.every((b) => first.items.every((a) => a.id !== b.id)));
  const csv = await api("/api/request-export?snapshot=" + first.snapshot);
  assert.equal(csv.count, count);
  assert.equal(csv.csv.split("\r\n").filter(Boolean).length, count + 1);
  const provider = await api(
    "/api/provider-usage?source=gateway&providerId=_unknown",
  );
  const model = await api("/api/agent-usage?source=gateway&agent=claude");
  assert.equal(provider.summary.calls, count);
  assert.equal(model.summary.attempts, count);
  assert.equal(
    model.summary.usage.totalTokens,
    first.summary.usage.totalTokens,
  );
  await reconcileUsage(api, provider, model, count, count * 12);
  assert.equal(rawDigest(fixture.db), before);
  if (process.env.LOCAL_AGENT_HISTORY_REPORT) {
    writeFileSync(
      process.env.LOCAL_AGENT_HISTORY_REPORT,
      JSON.stringify(
        {
          schema: 1,
          binary,
          count,
          modelCount,
          originalRowsSha256: before,
          query,
          first,
          deep,
          csv,
          provider,
          model,
        },
        null,
        2,
      ) + "\n",
    );
  }
  if (process.env.LOCAL_AGENT_HISTORY_BASELINE) {
    const old = await start(
      process.env.LOCAL_AGENT_HISTORY_BASELINE,
      seed("baseline"),
      false,
    );
    const page = await old(route(query));
    assert.deepEqual(page.summary, first.summary);
    assert.deepEqual(page.items, first.items);
    const stableUsage = ({ asOf, sourceAsOf, ...value }) => value;
    assert.deepEqual(
      stableUsage(
        await old("/api/provider-usage?source=gateway&providerId=_unknown"),
      ),
      stableUsage(provider),
    );
    assert.deepEqual(
      stableUsage(await old("/api/agent-usage?source=gateway&agent=claude")),
      stableUsage(model),
    );
    for (const [base, firstPage, size] of [
      [providerRoute, provider, 40],
      [modelRoute, model, 20],
    ]) {
      if (!firstPage.nextCursor) continue;
      const version = firstPage.nextCursor.split(":")[0];
      const total = size === 40 ? count : model.total;
      for (const offset of new Set([
        size,
        Math.floor((total - 1) / size) * size,
      ])) {
        const route = `${base}&cursor=${encodeURIComponent(`${version}:${offset}`)}`;
        assert.deepEqual(
          stableUsage(await api(route)),
          stableUsage(await old(route)),
        );
      }
    }
    assert.equal(
      (await old("/api/request-export?snapshot=" + page.snapshot)).csv,
      csv.csv,
    );
    console.log(
      "通过：旧版服务独立对照，请求、供应商、模型统计及完整 CSV 字节一致。",
    );
  }
  const updated = { ...rows[3], usage: { ...rows[3].usage, totalTokens: 777 } };
  fixture.db
    .prepare("UPDATE model_observations SET value=? WHERE id=?")
    .run(JSON.stringify(updated), updated.id);
  fixture.db
    .prepare("DELETE FROM model_observations WHERE id=?")
    .run(rows[4].id);
  fixture.db
    .prepare(
      "INSERT INTO model_observations VALUES('broken','invalid',1,'{bad')",
    )
    .run();
  assert.equal((await api(route(frozen))).total, count);
  assert.deepEqual(
    await api("/api/request-export?snapshot=" + first.snapshot),
    csv,
  );
  const fresh = await api(route(query));
  assert.equal(fresh.total, count - 1);
  assert.equal(fresh.summary.usage.totalTokens, count * 12 + 777 - 24);
  assert.match(fresh.sourceError, /1 条历史观测记录/);
  const assertUsageAfterChange = async () => {
    const provider = await api(
      "/api/provider-usage?source=gateway&providerId=_unknown",
    );
    const model = await api("/api/agent-usage?source=gateway&agent=claude");
    assert.equal(provider.summary.calls, fresh.total);
    assert.equal(model.summary.attempts, fresh.total);
    assert.equal(
      provider.summary.usage.totalTokens,
      fresh.summary.usage.totalTokens,
    );
    assert.equal(
      model.summary.usage.totalTokens,
      fresh.summary.usage.totalTokens,
    );
    await reconcileUsage(
      api,
      provider,
      model,
      fresh.total,
      fresh.summary.usage.totalTokens,
    );
  };
  await assertUsageAfterChange();
  const after = rawDigest(fixture.db);
  const child = services[0];
  const closed = once(child, "exit");
  child.kill();
  await closed;
  api = await start(binary, fixture, testVault);
  assert.equal(
    (await api(route(query))).summary.usage.totalTokens,
    fresh.summary.usage.totalTokens,
  );
  await assertUsageAfterChange();
  assert.equal(rawDigest(fixture.db), after);
  console.log(
    `通过：${count} 条完整 SQL 汇总、跨页面 Token 对账、深分页、冻结导出、同 revision 更新、删除、坏记录隔离及重启；原始记录摘要不变。`,
  );
} finally {
  for (const child of services)
    if (child.exitCode === null) {
      const closed = once(child, "exit");
      child.kill();
      await closed;
    }
  for (const db of databases) db.close();
  rmSync(root, { recursive: true, force: true });
}
