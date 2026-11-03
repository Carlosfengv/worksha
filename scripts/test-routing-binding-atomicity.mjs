import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

const previous = process.argv.includes("--observe-previous");
const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const hash = (file) =>
  createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "routing-binding-atomicity-")),
);
const report = {
  passed: false,
  previousGapObservation: previous,
  binarySha256: hash(binary),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
  simulatedUpstreamCalls: 0,
};
const children = new Set();
const upstream = http.createServer(async (request, response) => {
  let raw = "";
  for await (const chunk of request) raw += chunk;
  const body = JSON.parse(raw);
  report.simulatedUpstreamCalls++;
  response.writeHead(200, { "content-type": "application/json" });
  response.end(
    JSON.stringify({
      id: randomUUID(),
      type: "message",
      role: "assistant",
      model: body.model,
      content: [{ type: "text", text: "OK" }],
      stop_reason: "end_turn",
      usage: { input_tokens: 5, output_tokens: 2 },
    }),
  );
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));

function sql(context, action) {
  const db = new DatabaseSync(path.join(context.data, "local-agent.sqlite3"));
  try {
    return action(db);
  } finally {
    db.close();
  }
}
function counts(context) {
  return sql(context, (db) =>
    Object.fromEntries(
      [
        "gateway_bindings",
        "gateway_route_versions",
        "model_service_bindings",
      ].map((table) => [
        table,
        db.prepare(`SELECT count(*) n FROM ${table}`).get().n,
      ]),
    ),
  );
}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const done = once(child, "exit");
  const timer = setTimeout(() => child.kill("SIGKILL"), 8000);
  timer.unref();
  child.kill("SIGTERM");
  try {
    await done;
  } finally {
    clearTimeout(timer);
  }
}
async function api(
  context,
  route,
  body,
  method = body === undefined ? "GET" : "POST",
  headers = {},
) {
  const response = await fetch(context.base + route, {
    method,
    headers: {
      Authorization: "Bearer " + context.token,
      "content-type": "application/json",
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(15000),
  });
  return { status: response.status, body: await response.json() };
}
async function ok(context, route, body, method) {
  const result = await api(context, route, body, method);
  assert.equal(result.status, 200, JSON.stringify(result));
  return result.body;
}
async function fixture(name) {
  const home = path.join(root, name, "home"),
    data = path.join(root, name, "data");
  fs.mkdirSync(path.join(home, ".claude"), { recursive: true });
  const file = path.join(home, ".claude/settings.json");
  fs.writeFileSync(
    file,
    JSON.stringify({
      env: { KEEP_ME: "yes" },
      permissions: { allow: ["Read"] },
    }) + "\n",
    { mode: 0o600 },
  );
  const context = {
    home,
    data,
    file,
    sourceHash: hash(file),
    token: randomUUID(),
  };
  const child = spawn(binary, ["--data-dir", data], {
    env: {
      ...process.env,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: context.token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.add(child);
  child.once("exit", () => children.delete(child));
  context.child = child;
  let stderr = "";
  child.stderr.on("data", (bytes) => {
    stderr += bytes;
  });
  const lines = createInterface({ input: child.stdout });
  try {
    const [line] = await Promise.race([
      once(lines, "line", { signal: AbortSignal.timeout(15000) }),
      once(child, "exit").then(() => {
        throw Error(stderr);
      }),
    ]);
    context.base = "http://127.0.0.1:" + JSON.parse(line).port;
  } finally {
    lines.close();
  }
  const provider = {
    id: "custom-" + randomUUID(),
    vendor: "custom",
    name: "Atomic binding fixture",
    endpoint: "http://127.0.0.1:" + upstream.address().port,
    protocol: "messages",
    model: "fixture-model",
    models: ["fixture-model"],
    disabledModels: [],
    configured: true,
    credentialRef: null,
    credentialLabel: null,
    revision: 0,
    category: "local",
  };
  await ok(context, "/api/providers", provider, "PUT");
  context.provider = { ...provider, revision: 1 };
  assert.equal(
    (
      await ok(context, `/api/providers/${provider.id}/verify`, {
        expectedRevision: 1,
        model: provider.model,
      })
    ).state,
    "verified",
  );
  const modelId = randomUUID();
  context.service = await ok(
    context,
    "/api/model-services",
    {
      id: randomUUID(),
      revision: 0,
      name: "Atomic service",
      defaultModelId: modelId,
      models: [
        {
          id: modelId,
          name: "Default",
          providerId: provider.id,
          model: provider.model,
          capabilities: {
            contextWindow: 32768,
            maxOutputTokens: 8192,
            tools: true,
            thinking: false,
            inputModalities: ["text"],
          },
        },
      ],
    },
    "PUT",
  );
  context.config = (await ok(context, "/api/provider-status")).configs.find(
    (v) => v.agentId === "claude",
  );
  return context;
}
function connectInput(context, service) {
  return service
    ? {
        agentId: "claude",
        expectedRevision: context.config.revision,
        serviceId: context.service.id,
        expectedServiceRevision: context.service.revision,
      }
    : {
        agentId: "claude",
        expectedRevision: context.config.revision,
        providerId: context.provider.id,
        expectedProviderRevision: 1,
        model: context.provider.model,
      };
}
try {
  for (const [name, service, trigger] of [
    [
      "ignored-attachment",
      true,
      "CREATE TRIGGER fault BEFORE INSERT ON model_service_bindings BEGIN SELECT RAISE(IGNORE); END;",
    ],
    [
      "rewritten-binding",
      false,
      "CREATE TRIGGER fault AFTER INSERT ON gateway_bindings BEGIN UPDATE gateway_bindings SET credential='00000000-0000-4000-8000-000000000000' WHERE id=NEW.id; END;",
    ],
    [
      "rewritten-version",
      false,
      "CREATE TRIGGER fault AFTER INSERT ON gateway_route_versions BEGIN UPDATE gateway_route_versions SET value=json_set(value,'$.provider.model','rewritten') WHERE id=NEW.id; END;",
    ],
    [
      "attachment-rewrites-binding",
      true,
      "CREATE TRIGGER fault AFTER INSERT ON model_service_bindings BEGIN UPDATE gateway_bindings SET credential='00000000-0000-4000-8000-000000000000' WHERE id=NEW.binding; END;",
    ],
  ]) {
    const context = await fixture(name);
    sql(context, (db) => db.exec(trigger));
    const result = await api(
      context,
      service ? "/api/model-service-connections" : "/api/dynamic-connections",
      connectInput(context, service),
    );
    const after = counts(context);
    if (previous) {
      assert.equal(after.gateway_bindings, 1);
      assert.equal(after.gateway_route_versions, 1);
      assert.equal(
        after.model_service_bindings,
        service && name !== "ignored-attachment" ? 1 : 0,
      );
    } else {
      assert.equal(result.status, 503, JSON.stringify(result));
      assert.deepEqual(after, {
        gateway_bindings: 0,
        gateway_route_versions: 0,
        model_service_bindings: 0,
      });
    }
    assert.equal(hash(context.file), context.sourceHash);
    report.cases.push({
      name,
      status: result.status,
      persisted: after,
      originalNativeConfigUnchanged: true,
    });
    await stop(context.child);
  }
  if (!previous) {
    const context = await fixture("normal-retry");
    sql(context, (db) =>
      db.exec(
        "CREATE TRIGGER fault BEFORE INSERT ON model_service_bindings BEGIN SELECT RAISE(ABORT,'fixture failure'); END;",
      ),
    );
    assert.equal(
      (
        await api(
          context,
          "/api/model-service-connections",
          connectInput(context, true),
        )
      ).status,
      503,
    );
    assert.deepEqual(counts(context), {
      gateway_bindings: 0,
      gateway_route_versions: 0,
      model_service_bindings: 0,
    });
    sql(context, (db) => db.exec("DROP TRIGGER fault"));
    const pending = (await ok(context, "/api/local-tokens/protocol")).pending;
    assert.equal(pending.length, 1);
    const identity = pending[0].request.identity;
    const beforeContinuation = await ok(context, "/api/local-tokens/query", identity);
    assert.equal(beforeContinuation.credential.status, "ready");
    const recovered = await ok(
      context,
      "/api/local-tokens/continue",
      identity,
    );
    const transaction = recovered.preview;
    assert.equal(recovered.operation.credentialRef, beforeContinuation.operation.credentialRef);
    assert.equal(transaction.id, identity.operationId);
    assert.equal(transaction.state, "pending");
    assert.deepEqual(counts(context), {
      gateway_bindings: 1,
      gateway_route_versions: 1,
      model_service_bindings: 1,
    });
    const binding = sql(context, (db) =>
      db
        .prepare("SELECT * FROM gateway_bindings WHERE id=?")
        .get(transaction.routing.id),
    );
    const attachment = sql(context, (db) =>
      db
        .prepare("SELECT * FROM model_service_bindings WHERE binding=?")
        .get(binding.id),
    );
    assert.equal(attachment.service, context.service.id);
    assert.equal(attachment.version, context.service.versionId);
    assert.equal(binding.scope, context.config.path);
    assert.equal(hash(context.file), context.sourceHash);
    report.cases.push({
      name: "explicit-original-preview-after-failed-sql",
      originalNativeConfigUnchanged: true,
      oneCommittedBindingAndAttachment: true,
      originalOwnedTokenReused: true,
    });
    const before = counts(context);
    assert.equal(
      (
        await api(
          context,
          "/api/model-service-connections",
          connectInput(context, true),
          "POST",
          { Origin: "https://foreign.invalid" },
        )
      ).status,
      401,
    );
    assert.deepEqual(counts(context), before);
    report.cases.push({ name: "origin-denied-before-mutation", passed: true });
    await stop(context.child);
  }
  assert.equal(hash(binary), report.binarySha256);
  report.passed = true;
} finally {
  for (const child of [...children]) await stop(child);
  await new Promise((resolve) => upstream.close(resolve));
  report.allOwnedProcessesExited = children.size === 0;
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryRootRemoved = !fs.existsSync(root);
  if (process.env.ROUTING_BINDING_ATOMICITY_REPORT)
    fs.writeFileSync(
      process.env.ROUTING_BINDING_ATOMICITY_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
  console.log(JSON.stringify(report));
}
