import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID, createHash } from "node:crypto";
import {
  mkdtempSync,
  mkdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import http from "node:http";
import { once } from "node:events";
import path from "node:path";
import { tmpdir } from "node:os";
import { createInterface } from "node:readline";

const root = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-budget-")),
);
const home = path.join(root, "home");
mkdirSync(home);
const token = randomUUID();
const rootApi = "/api/agents/local-connections";
const cases = [];
const events = [];
let child;
let exit;
let management;
let mode = "ok";
let pausedAttempt;
let pausedRecovery;

const handleUpstream = async (req, res) => {
  let raw = "";
  for await (const chunk of req) raw += chunk;
  const request = JSON.parse(raw);
  events.push(request);
  const id = randomUUID();
  const tokens = mode === "large" ? 4900 : 7;
  const usage = {
    prompt_tokens: tokens - 2,
    completion_tokens: 2,
    total_tokens: tokens,
  };
  if (pausedAttempt && request.model === "primary-model") {
    const pause = pausedAttempt;
    pausedAttempt = null;
    pause.arrived();
    await pause.released;
    res.writeHead(429, {
      "Content-Type": "application/json",
      "x-request-id": id,
    });
    return res.end(
      JSON.stringify({
        error: { type: "rate_limit_error", message: "mock upstream limit" },
        usage,
      }),
    );
  }
  if (pausedRecovery && request.model === "fallback-model-policy") {
    const pause = pausedRecovery;
    pausedRecovery = null;
    pause.arrived();
    await pause.released;
  }
  if (mode === "uncertain") {
    res.writeHead(400, {
      "Content-Type": "application/json",
      "x-request-id": id,
    });
    return res.end(
      JSON.stringify({
        error: { type: "invalid_request_error", message: "fixture" },
      }),
    );
  }
  if (request.stream) {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "x-request-id": id,
    });
    const event = (choices, extra = {}) =>
      "data: " +
      JSON.stringify({
        id,
        object: "chat.completion.chunk",
        model: request.model,
        choices,
        ...extra,
      }) +
      "\n\n";
    return res.end(
      event([
        {
          index: 0,
          delta: { role: "assistant", content: "OK" },
          finish_reason: null,
        },
      ]) +
        event([{ index: 0, delta: {}, finish_reason: "stop" }]) +
        event([], { usage }) +
        "data: [DONE]\n\n",
    );
  }
  res.writeHead(200, {
    "Content-Type": "application/json",
    "x-request-id": id,
  });
  res.end(
    JSON.stringify({
      id,
      object: "chat.completion",
      model: request.model,
      choices: [
        {
          index: 0,
          message: { role: "assistant", content: "OK" },
          finish_reason: "stop",
        },
      ],
      usage,
    }),
  );
};
const upstream = http.createServer(handleUpstream);
const alternateUpstream = http.createServer(handleUpstream);

const digest = (key) => createHash("sha256").update(key).digest("hex");
const pair = () => {
  const key = randomUUID() + randomUUID();
  return { key, keyDigest: digest(key) };
};

async function start() {
  child = spawn(
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
      "target/debug/local-agent-service",
    ["--data-dir", path.join(root, "data")],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SOURCE_HOME: home,
        LOCAL_AGENT_SERVICE_TOKEN: token,
        LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
        LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  exit = once(child, "exit");
  let errors = "";
  child.stderr.on("data", (bytes) => {
    errors += bytes;
  });
  const ready = await Promise.race([
    once(createInterface({ input: child.stdout }), "line").then(([line]) =>
      JSON.parse(line),
    ),
    exit.then(() => {
      throw Error(errors);
    }),
  ]);
  management = `http://127.0.0.1:${ready.port}`;
}
async function api(
  route,
  value,
  status = 200,
  method = value === undefined ? "GET" : "POST",
) {
  const reply = await fetch(management + route, {
    method,
    headers: {
      authorization: "Bearer " + token,
      "content-type": "application/json",
    },
    body: value === undefined ? undefined : JSON.stringify(value),
    signal: AbortSignal.timeout(15_000),
  });
  const contents = await reply.text();
  assert.equal(reply.status, status, route + ": " + contents);
  return JSON.parse(contents);
}

async function stop() {
  await api("/api/runtime/shutdown", {});
  assert.equal((await exit)[0], 0);
}

async function wait(read, matches) {
  for (let tries = 0; tries < 40; tries++) {
    const result = await read();
    if (matches(result)) return result;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.fail("Budget transition did not finish");
}

try {
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  await new Promise((resolve) =>
    alternateUpstream.listen(0, "127.0.0.1", resolve),
  );
  await start();
  async function service(name, routingEnabled = false) {
    const server = name.startsWith("fallback") ? alternateUpstream : upstream;
    const provider = {
      id: `custom-${randomUUID()}`,
      name,
      vendor: "custom",
      protocol: "chat-completions",
      endpoint: `http://127.0.0.1:${server.address().port}/v1`,
      model: name,
      models: [name],
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
          model: name,
        })
      ).state,
      "verified",
    );
    const model = {
      id: randomUUID(),
      name,
      providerId: provider.id,
      model: name,
      capabilities: {
        contextWindow: 48000,
        maxOutputTokens: 8192,
        tools: true,
        thinking: false,
        inputModalities: ["text"],
      },
    };
    return api(
      "/api/model-services",
      {
        id: randomUUID(),
        revision: 0,
        name,
        defaultModelId: model.id,
        models: [model],
        ...(routingEnabled
          ? {
              routingPolicy: {
                enabled: true,
                order: [model.id],
                disabledModels: [],
                fallbackOnFailure: true,
                fallbackOnRateLimit: true,
                maxAttempts: 2,
                deadlineSeconds: 10,
                cooldownSeconds: 1,
                limits: [],
              },
            }
          : {}),
      },
      200,
      "PUT",
    );
  }
  const primary = await service("primary-model");
  const fallback = await service("fallback-model");
  const { storeId } = await api(rootApi);
  const operation = (action) => ({
    storeId,
    operationId: randomUUID(),
    action,
  });
  const query = ({ operationId }) => ({ storeId, operationId });
  const first = pair();
  const create = operation({
    kind: "create",
    name: "Budget fixture",
    serviceId: primary.id,
    serviceVersion: primary.versionId,
    keyDigest: first.keyDigest,
  });
  const saved = await api(rootApi + "/prepare", create);
  assert.equal(saved.receipt.state, "pending");
  assert(!JSON.stringify(saved).includes(first.keyDigest));
  assert.equal(
    (
      await fetch(saved.baseUrl + "/models", {
        headers: { authorization: "Bearer " + first.key },
      })
    ).status,
    401,
  );
  const cancelled = await api(rootApi + "/finish", {
    query: query(create),
    verifiedDigest: null,
  });
  assert.equal(cancelled.receipt.state, "cancelled");
  assert.equal(
    (
      await api(rootApi + "/finish", {
        query: query(create),
        verifiedDigest: first.keyDigest,
      })
    ).receipt.state,
    "cancelled",
  );
  const issue = operation(create.action);
  const prepared = await api(rootApi + "/prepare", issue);
  assert.equal(
    (await api(rootApi + "/operation", query(issue))).receipt.state,
    "pending",
  );
  const active = await api(rootApi + "/finish", {
    query: query(issue),
    verifiedDigest: first.keyDigest,
  });
  assert.equal(active.receipt.state, "active");
  assert.deepEqual(
    await api(rootApi + "/finish", {
      query: query(issue),
      verifiedDigest: first.keyDigest,
    }),
    active,
  );
  const { bindingId } = prepared.receipt;
  const url = active.baseUrl;
  const models = async (key, headers = {}) =>
    fetch(url + "/models", { headers: { "x-api-key": key, ...headers } });
  const chat = async (
    key,
    session,
    messages,
    status = 200,
    stream = false,
    target = url,
  ) => {
    const headers = {
      authorization: "Bearer " + key,
      "content-type": "application/json",
    };
    if (session) headers["x-local-agent-session-id"] = session;
    const reply = await fetch(target + "/chat/completions", {
      method: "POST",
      headers,
      body: JSON.stringify({
        model: "local-agent-default",
        messages,
        max_tokens: 32,
        stream,
      }),
      signal: AbortSignal.timeout(10_000),
    });
    const body = await reply.text();
    assert.equal(reply.status, status, body);
    return stream ? body : JSON.parse(body);
  };
  const user = (content) => ({ role: "user", content });
  assert.equal((await (await models(first.key)).json()).object, "list");
  assert.equal(
    (await models(first.key, { origin: "http://localhost" })).status,
    401,
  );
  const wrongHost = await new Promise((resolve, reject) => {
    http
      .get(
        url + "/models",
        { headers: { host: "evil.invalid", "x-api-key": first.key } },
        (response) => {
          response.resume();
          resolve(response.statusCode);
        },
      )
      .on("error", reject);
  });
  assert.equal(wrongHost, 401);
  assert.equal(
    (
      await fetch(management + new URL(url).pathname + "/models", {
        headers: { authorization: "Bearer " + token },
      })
    ).status,
    404,
  );
  cases.push(
    "pending and cancellation never activate; same operation receipt; scoped catalog, Host and Origin checks",
  );
  const budgetPath = `/api/agents/budgets/${bindingId}`;
  let policy = {
    revision: 0,
    timezone: "Asia/Shanghai",
    fallbackServiceId: fallback.id,
    warnPercent: 80,
    windows: [
      {
        kind: "daily",
        primary: { tokens: 5000, usdMicros: null },
        fallback: { tokens: 4000, usdMicros: null },
      },
    ],
  };
  policy = await api(budgetPath, { storeId, policy }, 200, "PUT");
  mode = "large";
  assert.equal(
    (await chat(first.key, "session-one", [user("first")])).choices[0].message
      .content,
    "OK",
  );
  assert.equal(events.at(-1).model, "primary-model");
  await wait(
    () => api(budgetPath),
    ({ windows }) =>
      windows.some(
        (w) => w.tier === "primary" && w.ledger.tokens.committed === 4900,
      ),
  );
  const warnings = await api("/api/agents/budget-warnings?pending=true");
  const crossed = warnings.warnings.filter(
    (warning) => warning.bindingId === bindingId && warning.tier === "primary",
  );
  assert.equal(crossed.length, 1);
  await api("/api/agents/budget-warnings", { storeId, ids: [crossed[0].id] });
  assert(
    !(await api("/api/agents/budget-warnings?pending=true")).warnings.some(
      (warning) => warning.id === crossed[0].id,
    ),
  );
  mode = "ok";
  const firstTurn = [
    user("first"),
    { role: "assistant", content: "OK" },
    { role: "user", content: "fresh next question" },
  ];
  const streamed = await chat(first.key, "session-one", firstTurn, 200, true);
  assert.match(streamed, /\[DONE\]/);
  assert.match(streamed, /主服务预算不足/);
  assert.equal(events.at(-1).model, "fallback-model");
  await wait(
    () => api(budgetPath),
    ({ windows }) =>
      windows.some(
        (w) => w.tier === "fallback" && w.ledger.tokens.committed === 7,
      ),
  );
  const tools = [
    ...firstTurn,
    {
      role: "assistant",
      content: null,
      tool_calls: [
        {
          id: "owned-call",
          type: "function",
          function: { name: "read", arguments: "{}" },
        },
      ],
    },
    { role: "tool", tool_call_id: "owned-call", content: "ready" },
  ];
  await stop();
  await start();
  await chat(first.key, "session-one", tools);
  assert.equal(events.at(-1).model, "fallback-model");
  const firstInNewSession = await chat(first.key, "session-new", [
    user("first request after the primary pool was spent"),
  ]);
  assert.match(firstInNewSession.choices[0].message.content, /主服务预算不足/);
  assert.equal(events.at(-1).model, "fallback-model");
  const before = events.length;
  const block = await chat(first.key, null, [user("unscoped")], 429);
  assert.equal(block.error.code, "local_agent_budget_exceeded");
  assert.equal(events.length, before);
  cases.push(
    "primary debit, next-turn fallback, full SSE usage, restart-sticky tool continuation, no-ID hard stop",
  );

  const old = first.key;
  const second = pair();
  const rotate = operation({
    kind: "rotate",
    bindingId,
    expectedKeyVersion: 1,
    keyDigest: second.keyDigest,
  });
  await api(rootApi + "/prepare", rotate);
  assert.equal((await models(old)).status, 200);
  assert.equal((await models(second.key)).status, 401);
  assert.equal(
    (
      await api(rootApi + "/finish", {
        query: query(rotate),
        verifiedDigest: second.keyDigest,
      })
    ).receipt.keyVersion,
    2,
  );
  assert.equal((await models(old)).status, 401);
  assert.equal((await models(second.key)).status, 200);
  assert.equal((await api(budgetPath)).policy.revision, policy.revision);
  cases.push(
    "rotation swaps key versions while retaining the budget and route binding",
  );

  policy = await api(
    budgetPath,
    {
      storeId,
      policy: {
        ...policy,
        fallbackServiceId: null,
        windows: [
          {
            kind: "daily",
            primary: { tokens: 15000, usdMicros: null },
            fallback: { tokens: null, usdMicros: null },
          },
        ],
      },
    },
    200,
    "PUT",
  );
  mode = "uncertain";
  await chat(second.key, "session-two", [user("no evidence")], 400);
  const frozen = (
    await wait(
      () => api(budgetPath),
      (data) => data.attempts.length === 1,
    )
  ).attempts[0];
  assert(frozen.upstreamRequestId && frozen.frozen.tokens > 0);
  assert.equal(
    (await api("/api/agents/budget-attempt/" + frozen.observationId)).attempt
      .attemptKey,
    frozen.attemptKey,
  );
  await stop();
  await start();
  assert.equal(
    (await api(budgetPath)).attempts[0].attemptKey,
    frozen.attemptKey,
  );
  const review = {
    storeId,
    operationId: randomUUID(),
    bindingId,
    attemptKey: frozen.attemptKey,
    expectedRevision: frozen.revision,
    upstreamRequestId: frozen.upstreamRequestId,
    source: "Fixture upstream request detail",
    tokens: { kind: "settle", amount: 9 },
    usdMicros: null,
  };
  const reviewed = await api("/api/agents/budget-review", review);
  assert.equal(reviewed.attempt.actual.tokens, 9);
  assert.deepEqual(await api("/api/agents/budget-review", review), reviewed);
  await api(
    "/api/agents/budget-review",
    { ...review, tokens: { kind: "release" } },
    409,
  );
  assert.equal((await api(budgetPath)).attempts.length, 0);
  assert.equal(
    (await api("/api/agents/budget-attempt/" + frozen.observationId)).attempt,
    null,
  );
  cases.push(
    "Uncertain survives restart, receipt settles actual usage once, conflicting review is denied",
  );

  await chat(
    second.key,
    "freeze-before-revoke",
    [user("revoked binding review")],
    400,
  );
  const freezeBeforeRevoke = (
    await wait(
      () => api(budgetPath),
      (data) => data.attempts.length === 1,
    )
  ).attempts[0];
  mode = "ok";
  const routeModels = [primary, fallback].map((service) => ({
    id: randomUUID(),
    name: service.name,
    providerId: service.models[0].provider.id,
    model: service.models[0].provider.model,
    capabilities: service.models[0].capabilities,
  }));
  const switching = await api(
    "/api/model-services",
    {
      id: randomUUID(),
      revision: 0,
      name: "Two candidates",
      defaultModelId: routeModels[0].id,
      models: routeModels,
      routingPolicy: {
        enabled: true,
        order: routeModels.map((model) => model.id),
        disabledModels: [],
        fallbackOnFailure: true,
        fallbackOnRateLimit: true,
        fallbackAcrossModels: true,
        maxAttempts: 2,
        deadlineSeconds: 10,
        cooldownSeconds: 1,
        limits: [],
      },
    },
    200,
    "PUT",
  );
  const third = pair();
  const issueRetry = operation({
    kind: "create",
    name: "Attempt fence fixture",
    serviceId: switching.id,
    serviceVersion: switching.versionId,
    keyDigest: third.keyDigest,
  });
  await api(rootApi + "/prepare", issueRetry);
  const retryBinding = await api(rootApi + "/finish", {
    query: query(issueRetry),
    verifiedDigest: third.keyDigest,
  });
  await api(
    `/api/agents/budgets/${retryBinding.receipt.bindingId}`,
    {
      storeId,
      policy: {
        revision: 0,
        timezone: "Asia/Shanghai",
        fallbackServiceId: null,
        warnPercent: 80,
        windows: [
          {
            kind: "daily",
            primary: { tokens: 10_000, usdMicros: null },
            fallback: { tokens: null, usdMicros: null },
          },
        ],
      },
    },
    200,
    "PUT",
  );
  assert.equal(
    (
      await fetch(retryBinding.baseUrl + "/models", {
        headers: { authorization: "Bearer " + second.key },
      })
    ).status,
    401,
  );
  const retryBudget = `/api/agents/budgets/${retryBinding.receipt.bindingId}`;
  function holdFirstCandidate(key, session, status) {
    const arrived = Promise.withResolvers();
    const released = Promise.withResolvers();
    pausedAttempt = { arrived: arrived.resolve, released: released.promise };
    const request = chat(
      key,
      session,
      [user("attempt fixture")],
      status,
      false,
      retryBinding.baseUrl,
    );
    return {
      request,
      release: released.resolve,
      ready: Promise.race([
        arrived.promise,
        request.then(() => {
          throw Error(
            "First upstream attempt returned before the fixture pause",
          );
        }),
      ]),
    };
  }
  const fourth = pair();
  const sentBefore = events.length;
  const oldRequest = holdFirstCandidate(third.key, "candidate-fence", 401);
  try {
    await oldRequest.ready;
    const rotateRetry = operation({
      kind: "rotate",
      bindingId: retryBinding.receipt.bindingId,
      expectedKeyVersion: 1,
      keyDigest: fourth.keyDigest,
    });
    await api(rootApi + "/prepare", rotateRetry);
    await api(rootApi + "/finish", {
      query: query(rotateRetry),
      verifiedDigest: fourth.keyDigest,
    });
  } finally {
    oldRequest.release();
  }
  await oldRequest.request;
  assert.equal(events.length - sentBefore, 1);
  await wait(
    () => api(retryBudget),
    (data) => data.windows.some((w) => w.ledger.tokens.committed === 7),
  );
  cases.push(
    "an HTTP error with reported usage settles the first attempt; rotation fences off its later candidate",
  );

  // Let the configured one-second upstream rate-limit cooldown elapse.
  await new Promise((resolve) => setTimeout(resolve, 1100));
  const beforePair = events.length;
  const twoCharged = holdFirstCandidate(fourth.key, "charged-attempts", 200);
  try {
    await twoCharged.ready;
  } finally {
    twoCharged.release();
  }
  assert.match((await twoCharged.request).choices[0].message.content, /OK/);
  assert.deepEqual(
    events.slice(beforePair).map((event) => event.model),
    ["primary-model", "fallback-model"],
  );
  await wait(
    () => api(retryBudget),
    (data) => data.windows.some((w) => w.ledger.tokens.committed === 21),
  );
  cases.push(
    "one request settles a charged rejection and its successful fallback separately",
  );

  await new Promise((resolve) => setTimeout(resolve, 1100));
  const beforeRevoke = events.length;
  const revokedRequest = holdFirstCandidate(
    fourth.key,
    "revoke-in-flight",
    401,
  );
  try {
    await revokedRequest.ready;
    const revokeRetry = operation({
      kind: "revoke",
      bindingId: retryBinding.receipt.bindingId,
      expectedKeyVersion: 2,
    });
    assert.equal(
      (await api(rootApi + "/prepare", revokeRetry)).receipt.state,
      "revoked",
    );
  } finally {
    revokedRequest.release();
  }
  await revokedRequest.request;
  assert.equal(events.length, beforeRevoke + 1);
  await wait(
    () => api(retryBudget),
    (data) => data.windows.some((w) => w.ledger.tokens.committed === 28),
  );

  const revoke = operation({
    kind: "revoke",
    bindingId,
    expectedKeyVersion: 2,
  });
  assert.equal(
    (await api(rootApi + "/prepare", revoke)).receipt.state,
    "revoked",
  );
  await stop();
  await start();
  assert.equal((await models(second.key)).status, 401);
  assert.equal((await api(budgetPath)).policy.revision, policy.revision);
  const releasedAfterRevoke = await api("/api/agents/budget-review", {
    ...review,
    operationId: randomUUID(),
    attemptKey: freezeBeforeRevoke.attemptKey,
    expectedRevision: freezeBeforeRevoke.revision,
    upstreamRequestId: freezeBeforeRevoke.upstreamRequestId,
    tokens: { kind: "release" },
  });
  assert.equal(releasedAfterRevoke.attempt.state, "released");
  cases.push(
    "revocation prevents later sends while an admitted attempt settles; frozen usage remains reviewable after restart and revocation",
  );
  mode = "ok";
  const configuredFallback = await service("fallback-model-policy", true);
  const last = pair();
  const pendingIssue = operation({
    kind: "create",
    name: "Pending route fixture",
    serviceId: primary.id,
    serviceVersion: primary.versionId,
    keyDigest: last.keyDigest,
  });
  await api(rootApi + "/prepare", pendingIssue);
  const pendingBinding = await api(rootApi + "/finish", {
    query: query(pendingIssue),
    verifiedDigest: last.keyDigest,
  });
  const pendingPath = `/api/agents/budgets/${pendingBinding.receipt.bindingId}`;
  let pendingPolicy = await api(
    pendingPath,
    {
      storeId,
      policy: {
        revision: 0,
        timezone: "Asia/Shanghai",
        fallbackServiceId: configuredFallback.id,
        warnPercent: 80,
        windows: [
          {
            kind: "session",
            primary: { tokens: 1, usdMicros: null },
            fallback: { tokens: 1, usdMicros: null },
          },
        ],
      },
    },
    200,
    "PUT",
  );
  const target = pendingBinding.baseUrl;
  const zero = events.length;
  assert.equal(
    (await chat(last.key, null, [user("no session")], 400, false, target)).error
      .code,
    "budget_session_required",
  );
  const prior = [user("unsent first question")];
  assert.equal(
    (await chat(last.key, "pending-sequence", prior, 429, false, target)).error
      .code,
    "budget_fallback_exhausted",
  );
  const newest = [
    ...prior,
    { role: "assistant", content: "fixture checkpoint" },
    user("unsent newer question"),
  ];
  assert.equal(
    (await chat(last.key, "pending-sequence", newest, 429, false, target)).error
      .code,
    "budget_fallback_exhausted",
  );
  assert.equal(
    (await chat(last.key, "pending-sequence", prior, 409, false, target)).error
      .code,
    "route_turn_superseded",
  );
  assert.equal(events.length, zero);
  pendingPolicy.windows[0].fallback.tokens = 5000;
  pendingPolicy = await api(
    pendingPath,
    { storeId, policy: pendingPolicy },
    200,
    "PUT",
  );
  await stop();
  await start();
  assert.equal(
    (await chat(last.key, "pending-sequence", prior, 409, false, target)).error
      .code,
    "route_turn_superseded",
  );
  const recoveryArrived = Promise.withResolvers();
  const releaseRecovery = Promise.withResolvers();
  pausedRecovery = {
    arrived: recoveryArrived.resolve,
    released: releaseRecovery.promise,
  };
  const recovering = chat(
    last.key,
    "pending-sequence",
    newest,
    200,
    false,
    target,
  );
  try {
    await Promise.race([
      recoveryArrived.promise,
      recovering.then(() => {
        throw Error("Recovered send missed the fixture pause");
      }),
    ]);
    const duplicate = await chat(
      last.key,
      "pending-sequence",
      newest,
      409,
      false,
      target,
    );
    assert.match(duplicate.error.message, /未重复发送/);
  } finally {
    releaseRecovery.resolve();
  }
  assert.match((await recovering).choices[0].message.content, /主服务预算不足/);
  assert.equal(events.at(-1).model, "fallback-model-policy");
  assert.equal(events.length, zero + 1);
  cases.push(
    "an older pending turn stays closed; the current one recovers with the backup policy and an overlapping retry makes no second send",
  );

  await stop();
  if (process.env.AGENT_BUDGET_REPORT) {
    writeFileSync(
      process.env.AGENT_BUDGET_REPORT,
      JSON.stringify(
        { cases, realModelCalls: 0, realKeychainCalls: 0 },
        null,
        2,
      ) + "\n",
    );
  }
  console.log(cases.join("\n"));
} finally {
  if (child?.exitCode === null) {
    try {
      await stop();
    } catch {
      child.kill("SIGTERM");
      await exit;
    }
  }
  upstream.close();
  alternateUpstream.close();
  rmSync(root, { recursive: true, force: true });
}
