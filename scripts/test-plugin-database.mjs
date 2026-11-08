import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const previous = process.env.PLUGIN_DATABASE_PREVIOUS_BINARY;
const rollback = process.env.PLUGIN_DATABASE_ROLLBACK_BINARY;
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "plugin-database-")),
);
const home = path.join(root, "home");
fs.mkdirSync(home);
const children = new Set();
const hash = (file) =>
  createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const report = {
  complete: false,
  binarySha256: hash(binary),
  previousBinarySha256: previous ? hash(previous) : null,
  rollbackBinarySha256: rollback ? hash(rollback) : null,
  actualDowngradeVerified: false,
  sameSchemaRollbackVerified: false,
  cases: [],
  realModelCalls: 0,
  realCredentialCalls: 0,
};
const passed = (name) => {
  report.cases.push(name);
  console.log("通过：" + name);
};
async function boot(data, executable = binary, rejected = false) {
  const token = randomUUID();
  const child = spawn(executable, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.add(child);
  let stderr = "";
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  const lines = createInterface({ input: child.stdout });
  let timer;
  try {
    const result = await Promise.race([
      once(lines, "line").then(([line]) => ({ ready: JSON.parse(line) })),
      once(child, "exit").then(([code]) => ({ code })),
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(Error("isolated service readiness timed out")),
          15000,
        );
      }),
    ]);
    if (rejected) {
      assert.equal(
        result.ready,
        undefined,
        "incompatible/damaged database must never become ready",
      );
      assert.notEqual(result.code, 0);
      children.delete(child);
      return stderr;
    }
    assert(result.ready, stderr);
    return { child, base: "http://127.0.0.1:" + result.ready.port, token };
  } finally {
    clearTimeout(timer);
    lines.close();
  }
}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) {
    children.delete(child);
    return;
  }
  const done = once(child, "exit");
  child.kill("SIGINT");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  await done.finally(() => {
    clearTimeout(timer);
    children.delete(child);
  });
}
async function api(
  service,
  route,
  body,
  method = body === undefined ? "GET" : "POST",
) {
  const response = await fetch(service.base + route, {
    method,
    headers: {
      Authorization: "Bearer " + service.token,
      "Content-Type": "application/json",
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(10000),
  });
  return { status: response.status, body: await response.json() };
}
async function changeComposition(service, update) {
  const { status, body: catalog } = await api(service, "/api/builtin-plugins");
  assert.equal(status, 200);
  const request = {
    operationId: randomUUID(),
    storeId: catalog.storeId,
    expectedRevision: catalog.revision,
    expectedGeneration: catalog.host.generation,
    composition: { ...catalog.composition, ...update },
    expectedImpact: catalog.impact,
  };
  const changed = await api(service, "/api/builtin-plugins/changes", request);
  assert.equal(changed.status, 200, JSON.stringify(changed.body));
  return { request, ...changed.body };
}
function saveFixtureProvider(service) {
  return api(
    service,
    "/api/providers",
    {
      id: "custom-" + randomUUID(),
      name: "Offline migration fixture",
      vendor: "custom",
      endpoint: "http://127.0.0.1:9/v1",
      protocol: "responses",
      model: "fixture-model",
      models: ["fixture-model"],
      configured: false,
      credentialRef: null,
      credentialLabel: null,
      revision: 0,
      category: "local",
    },
    "PUT",
  );
}
function sql(data, action) {
  const db = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  try {
    return action(db);
  } finally {
    db.close();
  }
}
function evidence(data) {
  return sql(data, (db) => ({
    version: db.prepare("PRAGMA user_version").get().user_version,
    providers: db.prepare("SELECT * FROM providers ORDER BY id").all(),
    sources: db.prepare("SELECT * FROM native_sources ORDER BY agent_id").all(),
    identity: db.prepare("SELECT * FROM model_service_operation_scope").all(),
    credentials: db.prepare("SELECT * FROM credential_recovery_scope").all(),
    state: db.prepare("SELECT * FROM builtin_plugin_composition").all(),
    receipts: db
      .prepare("SELECT * FROM builtin_plugin_operations ORDER BY sequence")
      .all(),
  }));
}
try {
  const data = path.join(root, "upgrade");
  // Simulate schema 15 with historical composition evidence. The actual older
  // process below predates the composition API and seeds its own database.
  let current = await boot(data);
  const savedProvider = await saveFixtureProvider(current);
  assert.equal(savedProvider.status, 200, JSON.stringify(savedProvider.body));
  fs.mkdirSync(path.join(home, ".codex", "sessions"), { recursive: true });
  assert.equal(
    (
      await api(
        current,
        "/api/session-sources",
        { agentId: "codex", enabled: true },
        "PUT",
      )
    ).status,
    200,
  );
  const catalog = (await api(current, "/api/builtin-plugins")).body;
  const request = {
    operationId: randomUUID(),
    storeId: catalog.storeId,
    expectedRevision: catalog.revision,
    expectedGeneration: catalog.host.generation,
    composition: {
      profile: "desktop",
      disabledPluginIds: ["builtin.projects"],
      removedPluginIds: ["builtin.projects"],
    },
    expectedImpact: catalog.impact,
  };
  const changed = await api(current, "/api/builtin-plugins/changes", request);
  assert.equal(changed.status, 200);
  const receipt = changed.body.receipt;
  await stop(current.child);
  sql(data, (db) => db.exec("PRAGMA user_version=15"));
  const before = evidence(data);
  assert.equal(before.version, 15);
  assert.equal(before.providers.length, 1);
  assert(
    before.sources.some(
      (source) => source.agent_id === "codex" && source.enabled === 1,
    ),
  );
  current = await boot(data);
  const after = evidence(data);
  assert.deepEqual(after, { ...before, version: 16 });
  const active = (await api(current, "/api/builtin-plugins")).body;
  assert.equal(active.storeId, catalog.storeId);
  assert.deepEqual(active.composition, request.composition);
  assert.equal(
    active.host.plugins.find((p) => p.id === "builtin.projects").runtimeActive,
    false,
  );
  const queried = await api(current, "/api/builtin-plugins/operations", {
    storeId: request.storeId,
    operationId: request.operationId,
  });
  assert.deepEqual(queried.body.receipt, receipt);
  passed(
    "15→16 preserves exact plugin state, receipts, identity, Provider and credential metadata; removed functionality stays stopped",
  );
  const backupFile = path.join(
    data,
    "local-agent.pre-plugin-lifecycle-v15.sqlite3",
  );
  const backup = new DatabaseSync(backupFile, { readOnly: true });
  try {
    assert.equal(backup.prepare("PRAGMA user_version").get().user_version, 15);
    assert.equal(backup.prepare("PRAGMA quick_check").get().quick_check, "ok");
    assert.deepEqual(
      backup.prepare("SELECT * FROM builtin_plugin_composition").all(),
      before.state,
    );
    assert.deepEqual(
      backup
        .prepare("SELECT * FROM builtin_plugin_operations ORDER BY sequence")
        .all(),
      before.receipts,
    );
  } finally {
    backup.close();
  }
  assert.equal(fs.statSync(backupFile).mode & 0o777, 0o600);
  passed(
    "pre-upgrade private backup is valid and contains the exact original plugin state and receipts",
  );
  await stop(current.child);
  current = await boot(data);
  assert.deepEqual(
    (await api(current, "/api/builtin-plugins/changes", request)).body.receipt,
    receipt,
  );
  assert.deepEqual(evidence(data), after);
  passed(
    "original operation replays its committed receipt after upgrade/restart without a second state change",
  );
  const crashCatalog = (await api(current, "/api/builtin-plugins")).body;
  const crashRequest = {
    operationId: randomUUID(),
    storeId: crashCatalog.storeId,
    expectedRevision: crashCatalog.revision,
    expectedGeneration: crashCatalog.host.generation,
    composition: {
      ...crashCatalog.composition,
      disabledPluginIds: [
        "builtin.projects",
        "io.localagent.builtin.source.codex",
      ],
    },
    expectedImpact: crashCatalog.impact,
  };
  const probe = new DatabaseSync(path.join(data, "local-agent.sqlite3"));
  try {
    probe.exec(
      "PRAGMA busy_timeout=0; CREATE TRIGGER owned_pause_receipt BEFORE INSERT ON builtin_plugin_operations BEGIN SELECT sum(i) FROM (WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<100000000) SELECT i FROM n); END;",
    );
    const pending = api(
      current,
      "/api/builtin-plugins/changes",
      crashRequest,
    ).catch(() => null);
    let locked = false;
    const deadline = Date.now() + 3000;
    while (!locked && Date.now() < deadline) {
      try {
        probe.exec("BEGIN IMMEDIATE; ROLLBACK;");
      } catch (error) {
        if (/locked|busy/i.test(error.message)) locked = true;
        else throw error;
      }
      if (!locked) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    assert(
      locked,
      "the actual composition transaction did not acquire its write lock",
    );
    await new Promise((resolve) => setTimeout(resolve, 100));
    const exited = once(current.child, "exit");
    current.child.kill("SIGKILL");
    await exited;
    children.delete(current.child);
    await pending;
    probe.exec("DROP TRIGGER owned_pause_receipt;");
  } finally {
    probe.close();
  }
  assert.deepEqual(evidence(data), after);
  current = await boot(data);
  assert.deepEqual(evidence(data), after);
  assert.equal(
    (
      await api(current, "/api/builtin-plugins/operations", {
        storeId: crashRequest.storeId,
        operationId: crashRequest.operationId,
      })
    ).body.receipt,
    null,
  );
  report.actualCrashRollbackVerified = true;
  passed(
    "forced termination inside the actual composition transaction preserves previous configuration, granted source and receipts across restart without replay",
  );
  await stop(current.child);
  sql(data, (db) => db.exec("DELETE FROM builtin_plugin_composition"));
  assert.match(await boot(data, binary, true), /原记录已保留/);
  assert.deepEqual(evidence(data), { ...after, state: [] });
  passed(
    "missing committed configuration blocks startup instead of silently enabling every plugin; receipts remain intact",
  );
  sql(data, (db) => {
    db.prepare(
      "INSERT INTO builtin_plugin_composition(id,value) VALUES(1,?)",
    ).run(after.state[0].value);
    db.exec("DROP TABLE builtin_plugin_operations");
  });
  assert.match(await boot(data, binary, true), /内置插件存储不完整/);
  assert.equal(
    sql(
      data,
      (db) =>
        db
          .prepare(
            "SELECT count(*) AS n FROM sqlite_master WHERE name='builtin_plugin_operations'",
          )
          .get().n,
    ),
    0,
  );
  passed(
    "missing current-format plugin tables are preserved as an error and never recreated as an empty registry",
  );
  const walData = path.join(root, "wal-upgrade");
  current = await boot(walData);
  await stop(current.child);
  const held = new DatabaseSync(path.join(walData, "local-agent.sqlite3"));
  try {
    held.exec(
      "PRAGMA wal_autocheckpoint=0; PRAGMA user_version=15; UPDATE providers SET value=json_set(value,'$.name','WAL migration fixture') WHERE id=(SELECT id FROM providers LIMIT 1)",
    );
    held
      .prepare("INSERT INTO providers(id,value) VALUES(?,?)")
      .run(before.providers[0].id, before.providers[0].value);
    assert(fs.statSync(path.join(walData, "local-agent.sqlite3-wal")).size > 0);
    const committed = held.prepare("SELECT * FROM providers ORDER BY id").all();
    assert.equal(committed.length, 1);
    current = await boot(walData);
    assert.deepEqual(
      sql(walData, (db) =>
        db.prepare("SELECT * FROM providers ORDER BY id").all(),
      ),
      committed,
    );
    const walBackup = new DatabaseSync(
      path.join(walData, "local-agent.pre-plugin-lifecycle-v15.sqlite3"),
      { readOnly: true },
    );
    try {
      assert.deepEqual(
        walBackup.prepare("SELECT * FROM providers ORDER BY id").all(),
        committed,
      );
    } finally {
      walBackup.close();
    }
    await stop(current.child);
  } finally {
    held.close();
  }
  passed(
    "upgrade backup includes committed WAL rows while a second connection remains open",
  );
  const inlineData = path.join(root, "inline-opt-in");
  current = await boot(inlineData);
  const inline = await changeComposition(current, {
    profile: "desktop",
    optedInPluginIds: ["builtin.projects"],
  });
  await stop(current.child);
  sql(inlineData, (db) =>
    db.exec(`
    UPDATE builtin_plugin_composition SET value=json_set(value,'$.composition.optedInPluginIds',json(opted_in));
    UPDATE builtin_plugin_operations SET request=json_set(request,'$.composition.optedInPluginIds',json(opted_in)),
      receipt=json_set(receipt,'$.composition.optedInPluginIds',json(opted_in));
    ALTER TABLE builtin_plugin_composition DROP COLUMN opted_in;
    ALTER TABLE builtin_plugin_operations DROP COLUMN opted_in;
  `),
  );
  current = await boot(inlineData);
  assert.deepEqual(
    (await api(current, "/api/builtin-plugins")).body.composition,
    inline.receipt.composition,
  );
  assert.deepEqual(
    (await api(current, "/api/builtin-plugins/changes", inline.request)).body
      .receipt,
    inline.receipt,
  );
  await stop(current.child);
  const migrated = evidence(inlineData);
  assert.equal(migrated.state[0].opted_in, '["builtin.projects"]');
  assert(
    !Object.hasOwn(
      JSON.parse(migrated.state[0].value).composition,
      "optedInPluginIds",
    ),
  );
  assert.equal(migrated.receipts[0].opted_in, '["builtin.projects"]');
  assert(
    !Object.hasOwn(
      JSON.parse(migrated.receipts[0].receipt).composition,
      "optedInPluginIds",
    ),
  );
  passed(
    "existing inline Projects choices and receipts migrate atomically to the rollback-compatible storage layout",
  );
  if (previous) {
    const oldData = path.join(root, "actual-schema15");
    const old = await boot(oldData, previous);
    assert.equal((await saveFixtureProvider(old)).status, 200);
    await stop(old.child);
    const original = sql(oldData, (db) => {
      assert.equal(db.prepare("PRAGMA user_version").get().user_version, 15);
      return db.prepare("SELECT * FROM providers").all();
    });
    current = await boot(oldData);
    await stop(current.child);
    const upgraded = evidence(oldData);
    assert.equal(upgraded.version, 16);
    assert.deepEqual(upgraded.providers, original);
    assert.match(await boot(oldData, previous, true), /数据库来自更新版本/);
    assert.deepEqual(evidence(oldData), upgraded);
    report.actualDowngradeVerified = true;
    passed(
      "actual schema-15 executable refuses the upgraded database without changing its rows",
    );
  }
  if (rollback) {
    const rollbackData = path.join(root, "same-schema-rollback");
    const old = await boot(rollbackData, rollback);
    assert.equal((await saveFixtureProvider(old)).status, 200);
    const older = await changeComposition(old, { profile: "desktop" });
    await stop(old.child);
    const original = evidence(rollbackData);
    assert.equal(original.version, 16);

    current = await boot(rollbackData);
    const opted = await changeComposition(current, {
      optedInPluginIds: ["builtin.projects"],
    });
    assert(
      opted.catalog.host.plugins.find((p) => p.id === "builtin.projects")
        .runtimeActive,
    );
    await stop(current.child);

    const reverted = await boot(rollbackData, rollback);
    const revertedCatalog = (await api(reverted, "/api/builtin-plugins")).body;
    assert.equal(revertedCatalog.revision, opted.receipt.revision);
    assert.equal(revertedCatalog.storeId, older.catalog.storeId);
    assert.deepEqual(evidence(rollbackData).providers, original.providers);
    const updated = await changeComposition(reverted, {
      disabledPluginIds: ["builtin.mcp"],
    });
    await stop(reverted.child);

    current = await boot(rollbackData);
    const restored = (await api(current, "/api/builtin-plugins")).body;
    assert.equal(restored.revision, updated.receipt.revision);
    assert.deepEqual(restored.composition.optedInPluginIds, [
      "builtin.projects",
    ]);
    assert.deepEqual(restored.composition.disabledPluginIds, ["builtin.mcp"]);
    assert(
      restored.host.plugins.find((p) => p.id === "builtin.projects")
        .runtimeActive,
    );
    const receipt = (request) =>
      api(current, "/api/builtin-plugins/operations", {
        storeId: request.storeId,
        operationId: request.operationId,
      });
    assert.deepEqual(
      (await receipt(older.request)).body.receipt,
      older.receipt,
    );
    assert.deepEqual(
      (await receipt(opted.request)).body.receipt,
      opted.receipt,
    );
    assert.deepEqual(
      (await receipt(updated.request)).body.receipt,
      updated.receipt,
    );
    assert.deepEqual(
      (await api(current, "/api/builtin-plugins/changes", opted.request)).body
        .receipt,
      opted.receipt,
    );
    await stop(current.child);
    report.sameSchemaRollbackVerified = true;
    passed(
      "same-schema rollback can write another plugin choice and upgrade again with Projects and all original receipts intact",
    );
  }
  report.complete = true;
} finally {
  await Promise.all([...children].map(stop));
  report.ownedProcessesExited = children.size === 0;
  fs.rmSync(root, { recursive: true, force: true });
  if (process.env.PLUGIN_DATABASE_REPORT)
    fs.writeFileSync(
      process.env.PLUGIN_DATABASE_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
