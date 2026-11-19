import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(
  process.env.PLUGIN_TEST_BINARY ?? "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "plugin-platform-")),
);
const data = path.join(root, "data");
const home = path.join(root, "home");
const source = path.join(home, ".claude", "projects", "fixture");
const key = path.join(root, "publisher.key");
const store = path.join(data, "plugins", "platform.sqlite3");
const main = path.join(data, "local-agent.sqlite3");
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], { encoding: "utf8" });
let token;
const timestamp = new Date(Date.now() - 1000).toISOString();
let base;
let child;
let upstream;
let exit;
let stderr = "";

function sql(file, run) {
  const db = new DatabaseSync(file);
  try {
    return run(db);
  } finally {
    db.close();
  }
}
const version = (file) =>
  sql(file, (db) => db.prepare("PRAGMA user_version").get().user_version);
async function api(route, body, expected = 200, session, method) {
  const response = await fetch(base + route, {
    method: method ?? (body === undefined ? "GET" : "POST"),
    headers: {
      Authorization: "Bearer " + token,
      "Content-Type": "application/json",
      ...(session ? { "x-local-agent-session": session } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20000),
  });
  const value = await response.json();
  assert(
    (Array.isArray(expected) ? expected : [expected]).includes(response.status),
    `${response.status} ${JSON.stringify(value)}`,
  );
  return value;
}
const platform = (route, body, status) =>
  api("/api/plugins/platform/" + route, body, status);
const catalog = async (plugin) =>
  (await platform("contributions")).items.find(
    ({ digest }) => digest === plugin.digest,
  );
const change = (plugin, action) =>
  api("/api/plugins/state", { ...plugin, action });
const manage = (plugin, operation, status) =>
  platform(
    "manage",
    {
      pluginId: plugin.pluginId,
      digest: plugin.digest,
      expectedGrantRevision: plugin.grantRevision,
      operation,
    },
    status,
  );
const query = (plugin, slotOrViewId = "board", context, status) =>
  platform("slot", { ...plugin, slotOrViewId, context }, status);
async function install(folder, rewrite, wasm) {
  const source = path.join(root, randomUUID());
  fs.cpSync(path.join("sdk/plugins/examples", folder), source, {
    recursive: true,
  });
  if (rewrite) {
    const file = path.join(source, "plugin.yaml");
    fs.writeFileSync(file, rewrite(fs.readFileSync(file, "utf8")));
  }
  if (wasm) fs.copyFileSync(wasm, path.join(source, "plugin.wasm"));
  const output = source + ".laplug";
  const packed = JSON.parse(sdk("pack", source, output, key));
  assert.equal(JSON.parse(sdk("check", output)).digest, packed.digest);
  const review = await api("/api/plugins/inspect", { path: output });
  const installed = await api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
  const plugin = { pluginId: installed.manifest.id, digest: installed.digest };
  await change(plugin, "enable");
  return plugin;
}
async function record(
  plugin,
  actionId,
  values = {},
  recordId,
  status = 200,
  expectedValues,
) {
  const current = await catalog(plugin);
  return platform(
    "action",
    {
      ...plugin,
      expectedEpoch: current.epoch,
      expectedGrantRevision: current.grantRevision,
      actionId,
      collection: "cards",
      values,
      recordId,
      expectedValues,
    },
    status,
  );
}
async function directory(plugin, purpose, path, status = 200) {
  const current = await catalog(plugin);
  return platform(
    "directory",
    {
      request: { ...plugin, purpose },
      path,
      expectedGrantRevision: current.grantRevision,
    },
    status,
  );
}
async function attempt(fn) {
  for (let n = 0; n < 36; n++) {
    const result = await fn();
    if (result) return result;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw Error("等待夹具发布超时");
}

try {
  fs.mkdirSync(source, { recursive: true });
  const nativeRows = [
    {
      type: "user",
      sessionId: "platform-native",
      uuid: "u",
      timestamp,
      cwd: root,
      message: { content: "本机夹具会话" },
    },
    {
      type: "assistant",
      sessionId: "platform-native",
      uuid: "a",
      timestamp,
      message: {
        id: "platform-reply",
        model: "fixture",
        content: [{ type: "text", text: "离线测试" }],
        usage: { input_tokens: 10, output_tokens: 3 },
      },
    },
  ];
  fs.writeFileSync(
    path.join(source, "platform-native.jsonl"),
    nativeRows.map((row) => JSON.stringify(row)).join("\n") + "\n",
  );
  sdk("keygen", key);
  const formats = {
    "plugin.jsonc": `// Normalize authoring formats before signing.
    {"manifest":{"manifestVersion":1,"id":"org.example.rule-formats","publisher":"org.example",
    "version":"1.0.0","displayName":"规则格式测试","hostApi":"^1.0.0","runtime":{"kind":"declarative"},
    "capabilities":["ui.platform@1",]},"platform":{"schemaVersion":1,},}`,
    "plugin.toml": `[manifest]
manifestVersion = 1
id = "org.example.rule-formats"
publisher = "org.example"
version = "1.0.0"
displayName = "规则格式测试"
hostApi = "^1.0.0"
capabilities = ["ui.platform@1"]
[manifest.runtime]
kind = "declarative"
[platform]
schemaVersion = 1
`,
  };
  const normalized = [];
  for (const [name, rules] of Object.entries(formats)) {
    const files = fs.mkdtempSync(path.join(root, "rule-format-"));
    fs.writeFileSync(path.join(files, name), rules);
    const archive = files + ".laplug";
    normalized.push(JSON.parse(sdk("pack", files, archive, key)).digest);
  }
  assert.equal(normalized[0], normalized[1]);
  const env = {
    ...process.env,
    LOCAL_AGENT_SOURCE_HOME: home,
    LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
    LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
  };
  const reservation = net.createServer().listen(0, "127.0.0.1");
  await once(reservation, "listening");
  const gatewayPort = reservation.address().port;
  await new Promise((resolve) => reservation.close(resolve));
  execFileSync(
    binary,
    [
      "gateway",
      "configure",
      "--data-dir",
      data,
      "--listen",
      `127.0.0.1:${gatewayPort}`,
      "--concurrency",
      "2",
    ],
    { env, stdio: "pipe" },
  );
  child = spawn(binary, ["gateway", "serve", "--data-dir", data], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  exit = once(child, "exit");
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(20000) }),
    exit.then(() => {
      throw Error(stderr);
    }),
  ]);
  lines.close();
  base = "http://127.0.0.1:" + JSON.parse(line).port;
  token = JSON.parse(
    fs.readFileSync(path.join(data, "runtime/service.json"), "utf8"),
  ).token;
  assert.equal(version(main), 18);

  const board = await install("session-kanban/package");
  const other = await install("session-kanban/package", (text) =>
    text.replace("org.example.session-kanban", "org.example.another-board"),
  );
  await api(
    "/api/session-sources",
    { agentId: "claude", enabled: true },
    200,
    undefined,
    "PUT",
  );
  const session = await attempt(async () =>
    (await api("/api/sessions")).items.find(
      (row) => row.nativeId === "platform-native",
    ),
  );
  const { recordId } = await record(board, "records.create", {
    title: "跑通测试",
    session: session.id,
    state: "待处理",
  });
  const selector = { target: "session", objectId: session.id };
  const cards = () => query(board, "session-card-list", selector);
  assert.equal(
    (await cards()).records[0]._hydrated.session.authorization,
    "unauthorized",
  );
  await manage(await catalog(board), {
    kind: "grant-entity",
    target: "session",
  });
  const authorized = (await cards()).records[0]._hydrated.session;
  assert.equal(authorized.authorization, "authorized");
  assert.equal(authorized.title, "本机夹具会话");
  assert.equal(authorized.totalTokens, 13);
  await record(board, "records.move", { state: "进行中" }, recordId);
  await record(
    board,
    "records.update",
    { title: "改名后的卡片" },
    recordId,
    200,
    {
      title: "跑通测试",
    },
  );
  assert.equal(
    (
      await record(
        board,
        "records.update",
        { title: "旧表单" },
        recordId,
        409,
        {
          title: "跑通测试",
        },
      )
    ).code,
    "record_conflict",
  );
  const patched = (await query(board)).records[0].values;
  assert.equal(patched.state, "进行中");
  assert.equal(patched.title, "改名后的卡片");
  assert.equal((await query(other)).records.length, 0);
  await change(board, "disable");
  await query(board, "board", undefined, 400);
  await change(board, "enable");
  assert.equal((await query(board)).records[0].recordId, recordId);
  assert.equal(version(store), 2);
  assert.equal(fs.statSync(store).mode & 0o777, 0o600);
  sql(store, (db) => {
    assert.equal(db.prepare("PRAGMA journal_mode").get().journal_mode, "wal");
    const plan = db
      .prepare(
        "EXPLAIN QUERY PLAN SELECT collection, record_id FROM ext_data_refs WHERE plugin_id=? AND target_kind=? AND target_id=?",
      )
      .all(board.pluginId, "session", session.id);
    assert(
      plan.some(({ detail }) => detail.includes("idx_ext_refs_lookup")),
      JSON.stringify(plan),
    );
    const insert = db.prepare("INSERT INTO ext_data_kv VALUES (?,?,?,?,?)");
    db.exec("BEGIN");
    for (let index = 0; index < 9999; index++)
      insert.run(board.pluginId, "fixture-" + index, "null", 4, timestamp);
    db.exec("COMMIT");
  });
  assert.equal(
    (
      await record(
        board,
        "records.create",
        { title: "超出行数", state: "待处理" },
        undefined,
        400,
      )
    ).code,
    "resource_limit",
  );
  assert.equal((await catalog(board)).usedRows, 10000);
  sql(store, (db) => {
    db.prepare("DELETE FROM ext_data_kv WHERE plugin_id=?").run(board.pluginId);
    db.prepare("INSERT INTO ext_data_kv VALUES (?,?,?,?,?)").run(
      board.pluginId,
      "capacity",
      "null",
      8 * 1048576,
      timestamp,
    );
  });
  assert.equal(
    (
      await record(
        board,
        "records.create",
        { title: "超出容量", state: "待处理" },
        undefined,
        400,
      )
    ).code,
    "resource_limit",
  );
  sql(store, (db) =>
    db.prepare("DELETE FROM ext_data_kv WHERE plugin_id=?").run(board.pluginId),
  );
  await record(board, "records.delete", {}, recordId);
  assert.equal((await cards()).records.length, 0);
  assert.equal(
    sql(
      store,
      (db) => db.prepare("SELECT COUNT(*) AS n FROM ext_data_refs").get().n,
    ),
    0,
  );

  const guest = await install(
    "session-kanban/package",
    (text) =>
      text
        .replace("org.example.session-kanban", "org.example.wasm-kanban")
        .replace(
          "{ kind: declarative }",
          "{ kind: wasm-component, entry: plugin.wasm, abi: local-agent:plugin@1.0.0 }",
        )
        .replace(
          "  schemaVersion: 1\n",
          "  schemaVersion: 1\n  wasm: { render: true, action: true }\n",
        ),
    ".work/plugin-platform-guest.wasm",
  );
  await manage(await catalog(guest), {
    kind: "grant-entity",
    target: "session",
  });
  const boardView = await query(guest);
  const operation = {
    ...guest,
    storeId: boardView.actionStoreId,
    operationId: randomUUID(),
    expectedEpoch: boardView.epoch,
    expectedGrantRevision: boardView.grantRevision,
    actionId: "records.create",
    collection: "cards",
    values: { title: "验证", session: session.id, state: "待处理" },
  };
  assert.deepEqual(
    await platform("action", { ...operation, receiptOnly: true }),
    { pending: true },
  );
  const receipts = await Promise.all([
    platform("action", operation),
    platform("action", operation),
  ]);
  assert.deepEqual(receipts[0], receipts[1]);
  assert.deepEqual(
    await platform("action", { ...operation, receiptOnly: true }),
    receipts[0],
  );
  assert.equal(
    (
      await platform(
        "action",
        { ...operation, values: { ...operation.values, title: "不同请求" } },
        409,
      )
    ).code,
    "operation_conflict",
  );
  await record(
    guest,
    "records.create",
    { title: "拒绝", state: "待处理" },
    undefined,
    403,
  );
  assert.equal((await query(guest)).records[0].values.title, "已核对 · 验证");
  assert.equal(
    sql(
      store,
      (db) =>
        db
          .prepare(
            "SELECT value_json FROM ext_data_kv WHERE plugin_id=? AND key='test-count'",
          )
          .get(guest.pluginId).value_json,
    ),
    "1",
  );

  const obs = path.join(data, "observation", "observations.sqlite3");
  const secret = "sk-platform-upstream-secret";
  const lan = "platform-client-secret-fingerprint";
  const observe = (id) =>
    sql(obs, (db) => {
      const row = {
        id,
        startedAt: timestamp,
        endedAt: timestamp,
        status: "success",
        agentId: "claude",
        model: secret,
        provider: "Synthetic gateway",
        endpoint: "https://example.invalid/" + secret,
        client: { id: lan, name: "Synthetic client", peer: "192.0.2.1" },
        bindingRevision: 1,
        durationMs: 1,
        protocol: "messages",
        revision: 1,
        usage: { inputTokens: 2, outputTokens: 3 },
      };
      db.prepare("INSERT INTO model_observations VALUES (?,?,?,?)").run(
        row.id,
        timestamp,
        1,
        JSON.stringify(row),
      );
    });
  observe("platform-request");
  let quotaArchive;
  for (const [example, suffix, exported, cursor] of [
    ["gateway-log-archive/package", ".jsonl", 1, "platform-request"],
    [
      "gateway-log-archive/otel-package",
      ".log",
      2,
      "platform-request-next.jsonl",
    ],
  ]) {
    const sink = await install(example);
    const target = fs.mkdtempSync(path.join(root, "archive-"));
    await directory(sink, "log-sink", target);
    const flush = (expected = 200) =>
      attempt(async () => {
        const result = await manage(
          await catalog(sink),
          { kind: "flush-sink" },
          [expected, 503],
        );
        return result.code === "busy" ? null : result;
      });
    const summary = await flush();
    assert.equal(summary.exportedRows, exported);
    const output = fs
      .readdirSync(target)
      .filter((name) => name.endsWith(suffix));
    assert.equal(output.length, 1);
    const body = fs.readFileSync(path.join(target, output[0]), "utf8");
    assert(!body.includes(secret) && !body.includes(lan));
    assert.equal((await flush()).exportedRows, exported);
    assert.equal(summary.quotaBytes, 512 * 1048576);
    sql(store, (db) =>
      db
        .prepare(
          "UPDATE ext_sink_grants SET quota_bytes=written_bytes WHERE plugin_id=?",
        )
        .run(sink.pluginId),
    );
    observe("platform-request-next" + suffix);
    assert.equal((await flush(400)).code, "resource_limit");
    const limited = (await catalog(sink)).sink;
    assert.equal(limited.lastError, "resource_limit");
    assert.equal(limited.exportedRows, exported);
    assert.deepEqual(fs.readdirSync(target), output);
    assert.equal(
      sql(
        store,
        (db) =>
          db
            .prepare(
              "SELECT last_request_id FROM ext_sink_cursors WHERE plugin_id=?",
            )
            .get(sink.pluginId).last_request_id,
      ),
      cursor,
    );
    fs.renameSync(target, target + "-previous");
    fs.mkdirSync(target);
    assert.equal((await flush(403)).code, "grant_revoked");
    await manage(await catalog(sink), {
      kind: "revoke-directory",
      purpose: "log-sink",
    });
    assert.equal((await flush(403)).code, "permission_denied");
    await api("/api/snapshot");
    if (suffix === ".jsonl") {
      quotaArchive = {
        sink,
        flush,
        target: target + "-previous",
        file: output[0],
        body,
      };
    }
  }

  const agent = await install("cursor-dir-agent/package");
  const unsafeSource = fs.mkdtempSync(path.join(root, "unsafe-mapper-"));
  fs.writeFileSync(
    path.join(unsafeSource, "plugin.yaml"),
    fs
      .readFileSync(
        "sdk/plugins/examples/cursor-dir-agent/package/plugin.yaml",
        "utf8",
      )
      .replace("**/agent-transcripts", "../agent-transcripts"),
  );
  assert.throws(() =>
    execFileSync(
      binary,
      ["--plugin-sdk", "pack", unsafeSource, unsafeSource + ".laplug", key],
      { stdio: "pipe" },
    ),
  );
  const directoryRoot = fs.mkdtempSync(path.join(root, "cursor-"));
  const transcripts = path.join(directoryRoot, "project", "agent-transcripts");
  fs.mkdirSync(transcripts, { recursive: true });
  const outside = fs.mkdtempSync(path.join(root, "outside-"));
  const hidden = path.join(outside, "private.jsonl");
  fs.writeFileSync(
    hidden,
    JSON.stringify({
      session_id: "outside",
      first_user_prompt: "private sentinel",
    }) + "\n",
  );
  fs.symlinkSync(hidden, path.join(transcripts, "symlink.jsonl"));
  fs.linkSync(hidden, path.join(transcripts, "hardlink.jsonl"));
  const currentAt = new Date().toISOString();
  const oldAt = new Date(Date.now() - 120_000).toISOString();
  for (const id of ["first", "second"]) {
    const at = id === "first" ? currentAt : oldAt;
    fs.writeFileSync(
      path.join(transcripts, id + ".jsonl"),
      JSON.stringify({
        session_id: id,
        first_user_prompt: id,
        workspace_root: path.join(directoryRoot, "project"),
        model_slug: "fixture-cursor-model",
        created_at: at,
        updated_at: at,
        usage: { input: 21, output: 3 },
        activity: { phase: id === "first" ? "generating" : "running", at },
        messages: Array.from(
          { length: id === "first" ? 35 : 1 },
          (_, index) => ({
            role: index % 2 === 0 ? "user" : "assistant",
            text: `directory-body-${id}-${index}`,
          }),
        ),
      }) + "\n",
    );
  }
  const alias = path.join(root, "alias");
  fs.symlinkSync(directoryRoot, alias);
  await directory(agent, "source-read", alias, 403);
  await directory(agent, "source-read", directoryRoot);
  const capabilities = [
    "domain.activity-source",
    "domain.project-source",
    "domain.session-source",
  ];
  const catalogEntries = (await api("/api/domain/capabilities")).entries.filter(
    ({ provider }) => provider.pluginId === agent.pluginId,
  );
  assert.deepEqual(
    catalogEntries.map(({ provider }) => provider.capability.name).sort(),
    capabilities,
  );
  const published = (await api("/api/snapshot")).agents.find(
    (item) => item.pluginId === agent.pluginId,
  );
  assert.deepEqual(
    [...published.capabilities].sort(),
    capabilities.map((name) => name + "@1"),
  );
  const malformed = path.join(transcripts, "broken.jsonl");
  fs.writeFileSync(malformed, "{");
  assert.equal(
    (await api("/api/domain/source-refresh", agent, 400)).code,
    "invalid_record",
  );
  const stale = (await api("/api/domain/capabilities")).entries.filter(
    ({ provider }) => provider.pluginId === agent.pluginId,
  );
  assert(
    stale.length && stale.every((entry) => entry.status.freshness === "stale"),
  );
  fs.unlinkSync(malformed);
  await api("/api/domain/source-refresh", agent);
  const domain = await api("/api/domain/directory");
  assert(JSON.stringify(domain).includes(agent.pluginId));
  const scope = { kind: "external", sourceIds: [agent.pluginId + ":source"] };
  const sessionQuery = {
    scope,
    filter: { query: null, productId: null, days: null },
    snapshotToken: null,
    cursor: null,
  };
  const mapped = await api("/api/domain/sessions", sessionQuery);
  assert.deepEqual(mapped.items.map(({ record }) => record.nativeId).sort(), [
    "first",
    "second",
  ]);
  const { record: first } = mapped.items.find(
    ({ record }) => record.nativeId === "first",
  );
  const { record: second } = mapped.items.find(
    ({ record }) => record.nativeId === "second",
  );
  assert.equal(first.usage.totalTokens, "24");
  assert.equal(Date.parse(first.updatedAt), Date.parse(currentAt));
  assert(!JSON.stringify(mapped).includes("directory-body"));
  const factQuery = {
    scope,
    kind: "project",
    sessionId: null,
    projectId: null,
    responseId: null,
    snapshotToken: null,
    cursor: null,
  };
  const projects = await api("/api/domain/facts", factQuery);
  assert.equal(projects.items.length, 1);
  assert.equal(projects.items[0].value.value.name, "project");
  assert.equal(projects.items[0].id, first.projectRef);
  assert.equal(first.projectRef, second.projectRef);
  const members = await api("/api/domain/facts", {
    ...factQuery,
    kind: "session",
    projectId: first.projectRef,
  });
  assert.deepEqual(
    members.items.map(({ id }) => id).sort(),
    [first.id, second.id].sort(),
  );
  const activityQuery = {
    scope,
    rangeSeconds: 600,
    productId: null,
    sessionId: null,
    includeGateway: false,
    includeHistorical: false,
  };
  const activity = await api("/api/domain/activity", activityQuery);
  const reported = activity.items.find(
    ({ sessionId }) => sessionId === first.id,
  );
  assert.equal(reported.state, "current");
  assert.equal(reported.phase, "generating");
  assert.equal(reported.model, "fixture-cursor-model");
  assert.equal(reported.measurements, null);
  assert.equal(
    activity.items.find(({ sessionId }) => sessionId === second.id).state,
    "expired",
  );
  const bodyQuery = {
    sessionId: first.id,
    snapshotToken: mapped.snapshotToken,
    cursor: null,
    anchor: null,
  };
  const bodyRoute = "/api/domain/conversation";
  const firstPage = (await api(bodyRoute, bodyQuery)).conversation;
  assert.equal(firstPage.events.length, 32);
  assert.equal(firstPage.events[0].text, "directory-body-first-0");
  assert.equal(firstPage.complete, false);
  assert.equal(firstPage.events[1].role, "assistant");
  assert.equal(
    (
      await api(
        bodyRoute,
        {
          ...bodyQuery,
          sessionId: second.id,
          cursor: firstPage.nextCursor,
        },
        409,
      )
    ).code,
    "stale_snapshot",
  );
  const lastPage = (
    await api(bodyRoute, { ...bodyQuery, cursor: firstPage.nextCursor })
  ).conversation;
  assert.equal(lastPage.complete, true);
  assert.deepEqual(
    lastPage.events.map(({ text }) => text),
    [32, 33, 34].map((index) => `directory-body-first-${index}`),
  );
  const anchored = (
    await api("/api/domain/fact-conversation", {
      ...bodyQuery,
      snapshotToken: members.snapshotToken,
      anchor: firstPage.events[2].id,
    })
  ).conversation;
  assert.equal(anchored.events[0].id, firstPage.events[2].id);
  const visibility = {
    sourceId: scope.sourceIds[0],
    projectId: first.projectRef,
  };
  await api("/api/domain/project-visibility", { ...visibility, hidden: true });
  const hiddenProjects = await api("/api/domain/facts", factQuery);
  assert.equal(hiddenProjects.items.length, 0);
  assert.equal(hiddenProjects.hiddenProjects[0].id, first.projectRef);
  assert.equal(
    (await api("/api/domain/sessions", sessionQuery)).items.length,
    0,
  );
  assert.equal((await api(bodyRoute, bodyQuery, 409)).code, "stale_snapshot");
  await api("/api/domain/project-visibility", { ...visibility, hidden: false });
  assert.equal(
    (await api("/api/domain/facts", factQuery)).items[0].id,
    first.projectRef,
  );
  const importedId = first.id;
  await record(board, "records.create", {
    title: "目录会话关联",
    session: importedId,
    state: "待处理",
  });
  const importedSummary = async () =>
    (
      await query(board, "session-card-list", {
        target: "session",
        objectId: importedId,
      })
    ).records[0]._hydrated.session;
  assert.equal((await importedSummary()).authorization, "unauthorized");
  await manage(await catalog(board), {
    kind: "grant-entity",
    target: "session",
  });
  const imported = await importedSummary();
  assert.equal(imported.authorization, "authorized");
  assert.equal(imported.model, "fixture-cursor-model");
  assert.equal(imported.sourceId, agent.pluginId + ":source");
  assert.equal(imported.totalTokens, 24);
  assert.equal(imported.activityState, "generating");
  const secondFile = path.join(transcripts, "second.jsonl");
  fs.rmSync(secondFile);
  fs.symlinkSync(hidden, secondFile);
  assert.equal(
    (await api(bodyRoute, { ...bodyQuery, sessionId: second.id }, 409)).code,
    "stale_snapshot",
  );
  await manage(await catalog(agent), {
    kind: "revoke-directory",
    purpose: "source-read",
  });
  const revoked = await importedSummary();
  assert.equal(revoked.authorization, "unknown");
  assert.equal(revoked.model, null);
  assert.equal(revoked.sourceId, undefined);
  await api(
    "/api/domain/project-visibility",
    { ...visibility, hidden: true },
    403,
  );
  assert.equal((await api(bodyRoute, bodyQuery, 409)).code, "stale_snapshot");
  assert.equal((await api("/api/domain/facts", factQuery)).items.length, 0);
  assert.equal(
    (await api("/api/domain/activity", activityQuery)).items.length,
    0,
  );
  await directory(agent, "source-read", directoryRoot);
  assert.equal((await api(bodyRoute, bodyQuery, 409)).code, "stale_snapshot");
  const restoredSessions = await api("/api/domain/sessions", sessionQuery);
  assert.deepEqual(
    restoredSessions.items.map(({ record }) => record.id),
    [first.id],
  );
  assert.equal(
    (
      await api(bodyRoute, {
        ...bodyQuery,
        snapshotToken: restoredSessions.snapshotToken,
      })
    ).conversation.events[0].text,
    "directory-body-first-0",
  );
  const adapter = await install("unified-provider/package");
  const catalogPolicy = await api("/api/provider-catalog/status", adapter);
  assert.equal(catalogPolicy.policy.path, "/catalog");
  const gatewayProtocol = await api("/api/plugin-extensions/status", adapter);
  assert.equal(gatewayProtocol.policy.protocol.baseProtocol, "responses");
  const quotaSecret = "platform-local-quota-fixture";
  let catalogCalls = 0;
  const modelCalls = [];
  upstream = http.createServer(async (request, response) => {
    if (request.headers.authorization !== "Bearer " + quotaSecret) {
      response.writeHead(401).end();
      return;
    }
    if (request.method === "GET" && request.url === "/v1/catalog") {
      catalogCalls++;
      response.writeHead(200, { "Content-Type": "application/json" }).end(
        JSON.stringify({
          entries: [
            {
              key: "fixture-model",
              label: "目录夹具模型",
              window: 32768,
              outputLimit: 4096,
            },
          ],
          balance: "12.5",
          limit: "50",
          unit: "credits",
          expiresAt: null,
        }),
      );
      return;
    }
    if (request.method === "POST" && request.url === "/v1/responses") {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      modelCalls.push(body);
      const completed = {
        id: "platform-composite-response",
        object: "response",
        status: "completed",
        model: "fixture-model",
        output: [
          {
            type: "message",
            id: "platform-composite-message",
            role: "assistant",
            content: [
              {
                type: "output_text",
                text: "COMPOSITE_OFFLINE",
                annotations: [],
              },
            ],
          },
        ],
        usage: { input_tokens: 3, output_tokens: 2, total_tokens: 5 },
      };
      response.writeHead(200, { "Content-Type": "text/event-stream" }).end(
        [
          {
            type: "response.output_text.delta",
            output_index: 0,
            delta: "COMPOSITE_OFFLINE",
          },
          { type: "response.completed", response: completed },
        ]
          .map((event) => `data: ${JSON.stringify(event)}\n\n`)
          .join(""),
      );
      return;
    }
    response.writeHead(404).end();
  });
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  const credential = await api("/api/provider-credentials", {
    label: "Platform fixture",
    secret: quotaSecret,
  });
  const provider = (await api("/api/snapshot")).providers.find(
    (row) => row.id === "openai",
  );
  await api(
    "/api/providers",
    {
      ...provider,
      ...credential,
      endpoint: `http://127.0.0.1:${upstream.address().port}/v1`,
      model: "fixture-model",
      models: ["fixture-model"],
      configured: true,
    },
    200,
    undefined,
    "PUT",
  );
  const saved = (await api("/api/snapshot")).providers.find(
    ({ id }) => id === "openai",
  );
  const target = {
    kind: "saved",
    providerId: saved.id,
    expectedRevision: String(saved.revision),
  };
  const catalogAction = (action, status) =>
    api("/api/provider-catalog/action", { ...adapter, action }, status);
  assert.equal(catalogCalls, 0);
  let binding = (
    await catalogAction({ kind: "bind", target, allowPrivateNetwork: false })
  ).bindings[0];
  const readCatalog = (status) =>
    catalogAction(
      { kind: "read", bindingId: binding.bindingId, target },
      status,
    );
  await readCatalog(403);
  assert.equal(catalogCalls, 0);
  binding = (
    await catalogAction({ kind: "bind", target, allowPrivateNetwork: true })
  ).bindings[0];
  const result = await readCatalog();
  assert.equal(catalogCalls, 1);
  assert.deepEqual(
    result.models.map(({ id, name, contextWindow, maxOutputTokens }) => ({
      id,
      name,
      contextWindow,
      maxOutputTokens,
    })),
    [
      {
        id: "fixture-model",
        name: "目录夹具模型",
        contextWindow: 32768,
        maxOutputTokens: 4096,
      },
    ],
  );
  assert.equal(new URL(result.sourceUrl).pathname, "/v1/catalog");
  assert(!JSON.stringify(result).includes(quotaSecret));
  assert.deepEqual(
    (await api("/api/snapshot")).providers.find(({ id }) => id === saved.id),
    saved,
  );

  await api("/api/plugins/action", {
    ...adapter,
    action: {
      kind: "bind-connection",
      providerId: "openai",
      allowPrivateNetwork: true,
    },
  });
  await api("/api/plugins/action", { ...adapter, action: { kind: "refresh" } });
  const quota = await api("/api/plugins/view", {
    pluginId: adapter.pluginId,
    viewId: "quota",
  });
  assert.equal(quota.quota.remaining, "12.5");
  assert.equal(catalogCalls, 2);
  assert(!JSON.stringify(quota).includes(quotaSecret));
  const protocolAction = (action) =>
    api("/api/plugin-extensions/action", { ...adapter, action });
  const bindProtocol = (allowPrivateNetwork) =>
    protocolAction({
      kind: "bind-protocol",
      providerId: saved.id,
      allowPrivateNetwork,
    });
  await bindProtocol(false);
  const client = await api("/api/gateway-lan/clients", {
    name: "Composite provider fixture",
    agent: "codex",
    providerId: saved.id,
    models: ["fixture-model"],
    concurrency: 2,
  });
  const modelCall = () =>
    fetch(`http://127.0.0.1:${gatewayPort}/v1/responses`, {
      method: "POST",
      headers: {
        Authorization: "Bearer " + client.key,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        model: "fixture-model",
        input: "synthetic platform response",
        stream: true,
      }),
      signal: AbortSignal.timeout(20000),
    });
  let reply = await modelCall();
  assert.equal(reply.status, 502);
  await reply.arrayBuffer();
  assert.equal(modelCalls.length, 0);
  await protocolAction({ kind: "unbind-protocol" });
  await bindProtocol(true);
  reply = await modelCall();
  assert.equal(reply.status, 200);
  const stream = await reply.text();
  assert.match(stream, /COMPOSITE_OFFLINE/);
  assert.match(stream, /"type":"response.completed"/);
  assert(!stream.includes(quotaSecret) && !stream.includes(client.key));
  assert.equal(modelCalls.length, 1);
  assert.equal(modelCalls[0].model, "fixture-model");
  assert.equal(modelCalls[0].stream, true);
  assert(
    JSON.stringify(modelCalls[0].input).includes("synthetic platform response"),
  );
  await attempt(async () =>
    (await api("/api/requests")).items.find(
      (row) => row.client?.id === client.id && row.status === "success",
    ),
  );
  await directory(quotaArchive.sink, "log-sink", quotaArchive.target);
  assert.equal((await quotaArchive.flush(400)).code, "resource_limit");
  reply = await modelCall();
  assert.equal(reply.status, 200);
  assert.match(await reply.text(), /COMPOSITE_OFFLINE/);
  assert.equal(modelCalls.length, 2);
  await attempt(async () => {
    const successes = (await api("/api/requests")).items.filter(
      (row) => row.client?.id === client.id && row.status === "success",
    );
    return successes.length === 2;
  });
  assert.equal((await quotaArchive.flush(400)).code, "resource_limit");
  sql(obs, (db) =>
    db
      .prepare("DELETE FROM model_observations WHERE id=?")
      .run("platform-request-next.jsonl"),
  );
  assert.equal((await quotaArchive.flush(409)).code, "source_changed");
  assert.equal(
    fs.readFileSync(path.join(quotaArchive.target, quotaArchive.file), "utf8"),
    quotaArchive.body,
  );
  assert.equal(version(main), 18);

  const matches = sql(
    store,
    (db) =>
      db.prepare("SELECT main_store_id FROM platform_meta").get().main_store_id,
  );
  sql(store, (db) =>
    db.prepare("UPDATE platform_meta SET main_store_id=?").run(randomUUID()),
  );
  assert.equal((await catalog(board)).diagnostic, "invalid_record");
  await api("/api/snapshot");
  sql(store, (db) =>
    db.prepare("UPDATE platform_meta SET main_store_id=?").run(matches),
  );
  const prepared = await api("/api/iam", { kind: "prepare", recovery: false });
  const password = "fixture-plugin-platform-owner";
  await api("/api/iam", {
    kind: "initialize",
    operationId: prepared.operationId,
    username: "owner",
    displayName: "Platform Fixture",
    password,
  });
  assert.equal(version(main), 19);
  await api("/api/plugins/platform/contributions", undefined, 401);
  const owner = await api("/api/iam", {
    kind: "login",
    username: "owner",
    password,
  });
  const afterIam = await api(
    "/api/plugins/platform/contributions",
    undefined,
    200,
    owner.session,
  );
  assert(
    afterIam.items.some(
      (item) => item.digest === board.digest && item.diagnostic === null,
    ),
  );
  assert.equal(version(store), 2);
  console.log(
    "通过：签名、隔离、授权、CRUD、配额、引用索引、两种日志格式、目录项目与活动、正文分页与撤权、复合 Provider 目录与协议往返、主库身份校验",
  );
} finally {
  upstream?.close();
  if (child?.exitCode === null && child.signalCode === null) {
    child.kill("SIGINT");
    const timeout = setTimeout(() => child.kill("SIGKILL"), 6000);
    await exit.finally(() => clearTimeout(timeout));
  }
  fs.rmSync(root, { recursive: true, force: true });
}
