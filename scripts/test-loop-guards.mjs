import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  mkdtempSync,
  mkdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";

const root = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-loop-")),
);
const home = path.join(root, "source-home");
mkdirSync(home);
const managementKey = randomUUID();
const key = randomUUID() + randomUUID();
const circuit = "local_agent_loop_circuit_breaker";
const capacity = "local_agent_loop_dedup_capacity";
const cases = [];
let child;
let exit;
let base;
let sends = 0;
let nextHeld;
let nextTokens;
const upstream = http.createServer(async (request, response) => {
  let raw = "";
  for await (const chunk of request) raw += chunk;
  const body = JSON.parse(raw);
  const id = randomUUID();
  sends++;
  const hold = nextHeld;
  nextHeld = null;
  if (hold) hold.arrived.resolve();
  const tokens = hold ? await hold.release.promise : (nextTokens ?? 7);
  nextTokens = undefined;
  response.writeHead(200, {
    "content-type": "application/json",
    "x-request-id": id,
  });
  response.end(
    JSON.stringify({
      id,
      object: "chat.completion",
      model: body.model,
      choices: [
        {
          index: 0,
          message: { role: "assistant", content: "fixture reply" },
          finish_reason: "stop",
        },
      ],
      usage: {
        prompt_tokens: tokens - 1,
        completion_tokens: 1,
        total_tokens: tokens,
      },
    }),
  );
});

async function start() {
  child = spawn(
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
      "target/debug/local-agent-service",
    ["--data-dir", path.join(root, "data")],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SOURCE_HOME: home,
        LOCAL_AGENT_SERVICE_TOKEN: managementKey,
        LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
        LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let stderr = "";
  child.stderr.on("data", (bytes) => {
    stderr += bytes;
  });
  exit = once(child, "exit");
  const ready = await Promise.race([
    once(createInterface({ input: child.stdout }), "line").then(([line]) =>
      JSON.parse(line),
    ),
    exit.then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${ready.port}`;
}
async function api(
  route,
  body,
  expected = 200,
  method = body === undefined ? "GET" : "POST",
  extraHeaders,
) {
  const response = await fetch(base + route, {
    method,
    signal: AbortSignal.timeout(12000),
    headers: {
      authorization: `Bearer ${managementKey}`,
      "content-type": "application/json",
      ...extraHeaders,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const payload = await response.text();
  assert.equal(response.status, expected, route + ": " + payload);
  return JSON.parse(payload);
}
async function stop() {
  await api("/api/runtime/shutdown", {});
  assert.equal((await exit)[0], 0);
}
const pathToGuard = "/api/observation/loop-guards";
const alerts = () => api(pathToGuard);
async function alert(session, expectedReason) {
  const info = await alerts();
  const item = info.alerts.find(
    ({ sessionId }) => sessionId === `chat:${session}`,
  );
  assert.equal(item?.reason, expectedReason, JSON.stringify(info.alerts));
  return item;
}
const user = (content) => ({ role: "user", content });
const assistant = { role: "assistant", content: "fixture checkpoint" };
function batch(history, id, { count = 1, error = false, action } = {}) {
  const calls = Array.from({ length: count }, (_, index) => ({
    id: `call_${id}_${index}`,
    type: "function",
    function: {
      name: "loop_fixture",
      arguments: JSON.stringify({ target: action ?? `${id}-${index}` }),
    },
  }));
  return [
    ...history,
    { role: "assistant", content: null, tool_calls: calls },
    ...calls.map((call) => ({
      role: "tool",
      tool_call_id: call.id,
      content: "error in plain text",
      ...(error ? { is_error: true } : {}),
    })),
  ];
}

try {
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  await start();
  const provider = {
    id: `custom-${randomUUID()}`,
    name: "Loop fixture",
    vendor: "custom",
    protocol: "chat-completions",
    endpoint: `http://127.0.0.1:${upstream.address().port}/v1`,
    model: "loop-model",
    models: ["loop-model"],
    configured: true,
    credentialRef: null,
    credentialLabel: null,
    revision: 0,
    category: "local",
  };
  await api("/api/providers", provider, 200, "PUT");
  assert.equal(
    (
      await api(`/api/providers/${provider.id}/verify`, {
        expectedRevision: 1,
        model: "loop-model",
      })
    ).state,
    "verified",
  );
  const model = {
    id: randomUUID(),
    name: "Loop fixture",
    providerId: provider.id,
    model: "loop-model",
    capabilities: {
      contextWindow: 400000,
      maxOutputTokens: 65536,
      tools: true,
      thinking: false,
      inputModalities: ["text"],
    },
  };
  const service = await api(
    "/api/model-services",
    {
      id: randomUUID(),
      revision: 0,
      name: "Loop fixture",
      defaultModelId: model.id,
      models: [model],
    },
    200,
    "PUT",
  );
  const connections = "/api/agents/local-connections";
  const { storeId } = await api(connections);
  const operation = { storeId, operationId: randomUUID() };
  await api(connections + "/prepare", {
    ...operation,
    action: {
      kind: "create",
      name: "Loop binding",
      serviceId: service.id,
      serviceVersion: service.versionId,
      keyDigest: createHash("sha256").update(key).digest("hex"),
    },
  });
  const active = await api(connections + "/finish", {
    query: operation,
    verifiedDigest: createHash("sha256").update(key).digest("hex"),
  });
  const { bindingId } = active.receipt;
  async function call(session, messages, maxTokens = 32) {
    const response = await fetch(active.baseUrl + "/chat/completions", {
      method: "POST",
      signal: AbortSignal.timeout(12000),
      headers: {
        authorization: `Bearer ${key}`,
        "content-type": "application/json",
        "x-local-agent-session-id": session,
      },
      body: JSON.stringify({
        model: "local-agent-default",
        messages,
        max_tokens: maxTokens,
        tools: [
          {
            type: "function",
            function: { name: "loop_fixture", parameters: { type: "object" } },
          },
        ],
      }),
    });
    return { status: response.status, body: await response.json() };
  }
  async function chat(session, messages, status = 200, code, maxTokens) {
    const result = await call(session, messages, maxTokens);
    assert.equal(result.status, status, JSON.stringify(result.body));
    if (code)
      assert.equal(result.body.error?.code, code, JSON.stringify(result.body));
    return result.body;
  }
  function override(item) {
    return {
      storeId,
      operationId: randomUUID(),
      bindingId,
      sessionId: item.sessionId,
      expectedTurnDigest: item.turnDigest,
      expectedOverrideEpoch: item.overrideEpoch,
    };
  }
  const budgetPath = `/api/agents/budgets/${bindingId}`;
  const policy = await api(
    budgetPath,
    {
      storeId,
      policy: {
        revision: 0,
        timezone: "Asia/Shanghai",
        warnPercent: 80,
        fallbackServiceId: null,
        windows: [
          {
            kind: "daily",
            primary: { tokens: 200, usdMicros: null },
            fallback: { tokens: null, usdMicros: null },
          },
        ],
      },
    },
    200,
    "PUT",
  );
  const a = [user("first user lease, initially blocked by budget")];
  const b = [...a, assistant, user("next trusted user message")];
  const beforeA = sends;
  await chat("older-lease", a, 429, "local_agent_budget_exceeded", 1000);
  assert.equal(sends, beforeA);
  await api(
    budgetPath,
    { storeId, policy: { ...policy, windows: [] } },
    200,
    "PUT",
  );
  await chat("older-lease", b);
  const failed = (i) => batch(b, `failure-${i}`, { error: true });
  await chat("older-lease", failed(0));
  await chat("older-lease", failed(1));
  await chat("older-lease", failed(0));
  await chat("older-lease", failed(2));
  let warning = await alert("older-lease", "toolErrorStreak");
  assert.equal(warning.status, "warned");
  assert.equal(warning.continuationSteps, 3);
  await chat("older-lease", failed(3));
  const beforeTrip = sends;
  await chat("older-lease", failed(4), 429, circuit);
  await chat("older-lease", a, 429, circuit);
  await chat("older-lease", [user("unrelated history")], 429, circuit);
  assert.equal(sends, beforeTrip);
  await stop();
  await start();
  await chat("older-lease", failed(4), 429, circuit);
  await chat("older-lease", a, 429, circuit);
  assert.equal(sends, beforeTrip);
  const tripped = await alert("older-lease", "toolErrorStreak");
  assert.equal(tripped.consecutiveToolErrors, 5);
  const input = override(tripped);
  await api(
    pathToGuard + "/override",
    { ...input, operationId: randomUUID(), expectedOverrideEpoch: 1 },
    409,
  );
  await api(pathToGuard + "/override", input, 401, "POST", {
    origin: "https://invalid.example",
  });
  const receipt = await api(pathToGuard + "/override", input);
  assert.equal(receipt.overrideEpoch, 1);
  assert.deepEqual(await api(pathToGuard + "/override", input), receipt);
  assert.equal(await alert("older-lease", undefined), undefined);
  await chat("older-lease", batch(b, "normal-proof"));
  for (let i = 5; i < 9; i++) await chat("older-lease", failed(i));
  await chat("older-lease", failed(9), 429, circuit);
  assert.equal(
    (await alert("older-lease", "toolErrorStreak")).overrideEpoch,
    1,
  );
  await chat("older-lease", [...b, assistant, user("trusted unlock")]);
  assert.equal(await alert("older-lease", undefined), undefined);
  cases.push(
    "A lease without a send cannot reopen B; A→B→A batch replay counts once; restart, override receipts and trusted unlock",
  );

  const same = [user("repeated action")];
  await chat("same-action", same);
  await chat(
    "same-action",
    batch(same, "parallel-four", { count: 4, action: "same" }),
  );
  warning = await alert("same-action", "actionPingPong");
  assert.equal(warning.continuationSteps, 1);
  assert.equal(warning.status, "warned");
  const beforeAction = sends;
  await chat(
    "same-action",
    batch(same, "parallel-two", { count: 2, action: "same" }),
    429,
    circuit,
  );
  assert.equal(sends, beforeAction);
  cases.push(
    "parallel calls count one step; plain-text error is treated as a successful tool return; repeated actions trip at six",
  );

  const crowded = [user("tool capacity")];
  await chat("tool-capacity", crowded);
  await chat("tool-capacity", batch(crowded, "initial-tools", { count: 511 }));
  const beforeRace = sends;
  const raced = await Promise.all([
    call("tool-capacity", batch(crowded, "last-a")),
    call("tool-capacity", batch(crowded, "last-b")),
  ]);
  assert(
    raced.some(
      ({ status, body }) => status === 429 && body.error?.code === capacity,
    ),
    JSON.stringify(raced),
  );
  assert(sends - beforeRace <= 1);
  const full = await alert("tool-capacity", "dedupCapacity");
  assert.equal(full.continuationSteps, 2);
  const deny = await api(pathToGuard + "/override", override(full), 409);
  assert.equal(deny.code, capacity);
  const fullSendCount = sends;
  await stop();
  await start();
  await chat(
    "tool-capacity",
    batch(crowded, "initial-tools", { count: 511 }),
    429,
    capacity,
  );
  assert.equal(sends, fullSendCount);
  await chat("tool-capacity", [
    ...crowded,
    assistant,
    user("fresh capacity set"),
  ]);
  await chat(
    "tool-capacity",
    batch(
      [...crowded, assistant, user("fresh capacity set")],
      "initial-tools",
      { count: 511 },
    ),
  );
  cases.push(
    "concurrent last tool slot; full guard survives restart, refuses manual override, and clears on a trusted next turn",
  );

  const spending = [user("late settlement")];
  const old = {
    arrived: Promise.withResolvers(),
    release: Promise.withResolvers(),
  };
  nextHeld = old;
  const flight = chat("epoch", spending, 200, undefined, 65000);
  try {
    await Promise.race([
      old.arrived.promise,
      flight.then(() => {
        throw Error("fixture did not hold the old send");
      }),
    ]);
    for (let i = 0; i < 4; i++)
      await chat("epoch", batch(spending, `epoch-${i}`, { error: true }), 409);
    await chat(
      "epoch",
      batch(spending, "epoch-4", { error: true }),
      429,
      circuit,
    );
    await api(
      pathToGuard + "/override",
      override(await alert("epoch", "toolErrorStreak")),
    );
  } finally {
    old.release.resolve(600000);
  }
  await flight;
  nextTokens = 195000;
  await chat("epoch", spending);
  const beforeLimit = sends;
  await chat("epoch", spending, 429, circuit, 60000);
  assert.equal(sends, beforeLimit);
  const burned = await alert("epoch", "unattendedBurn");
  assert.equal(burned.overrideEpoch, 1);
  assert.equal(burned.usage.committed.tokens, 195000);
  assert.equal(burned.usage.reserved.tokens, null);
  assert(burned.usage.usdUnknown > 0);
  await stop();
  await start();
  const finalSends = sends;
  await chat("epoch", spending, 429, circuit);
  assert.equal(sends, finalSends);
  cases.push(
    "late in-flight usage stays with the original epoch; a new epoch re-trips on reserved tokens and stays closed after restart",
  );
  await stop();
  const report = { cases, realModelCalls: 0, realKeychainCalls: 0 };
  if (process.env.LOOP_GUARD_REPORT)
    writeFileSync(
      process.env.LOOP_GUARD_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
  console.log(JSON.stringify(report));
} finally {
  if (child?.exitCode === null) {
    try {
      await stop();
    } catch {
      child.kill("SIGTERM");
      await exit;
    }
  }
  upstream.closeAllConnections();
  upstream.close();
  rmSync(root, { recursive: true, force: true });
}
