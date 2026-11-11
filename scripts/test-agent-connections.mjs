import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { createServer } from "node:http";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import fs from "node:fs/promises";
import path from "node:path";
import { createInterface } from "node:readline";
import * as bridge from "../sdk/plugins/examples/dsh-desktop/connection-bridge/index.js";

const directory = await fs.realpath(
  await fs.mkdtemp("/tmp/local-agent-connect-"),
);
const ownedSource = process.env.DSH_TEST_SOURCE_DIR;
const bundled = process.argv.includes("--bundled");
assert(!(bundled && ownedSource));
if (ownedSource) {
  assert.match(
    ownedSource,
    /^\/tmp\/worksha-dsh-control\.[A-Za-z0-9]+\/source$/,
  );
  assert.equal(
    await fs.realpath(ownedSource),
    path.join("/private", ownedSource),
  );
}
const home = path.join(directory, "home");
const source =
  ownedSource ??
  (bundled
    ? path.join(home, ".local/share/local-agent/dsh-desktop")
    : path.join(directory, "source"));
const binary = path.resolve(
  process.env.PLUGIN_TEST_BINARY ?? "target/debug/local-agent-service",
);
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], { stdio: "pipe" });
const token = randomUUID();
const traffic = [];
const upstream = createServer(async (request, response) => {
  try {
    let data = "";
    for await (const chunk of request) data += chunk;
    const body = JSON.parse(data);
    assert.equal(body.model, "test-model");
    assert.equal(request.url, "/v1/messages");
    traffic.push(body.model);
    if (body.stream) {
      const message = {
        id: randomUUID(),
        type: "message",
        role: "assistant",
        model: body.model,
        content: [],
        stop_reason: null,
        usage: { input_tokens: 5, output_tokens: 0 },
      };
      const events = [
        { type: "message_start", message },
        {
          type: "content_block_start",
          index: 0,
          content_block: { type: "text", text: "" },
        },
        {
          type: "content_block_delta",
          index: 0,
          delta: { type: "text_delta", text: "isolated reply" },
        },
        { type: "content_block_stop", index: 0 },
        {
          type: "message_delta",
          delta: { stop_reason: "end_turn", stop_sequence: null },
          usage: { output_tokens: 2 },
        },
        { type: "message_stop" },
      ];
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.end(
        events
          .map(
            (event) =>
              `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`,
          )
          .join(""),
      );
      return;
    }
    response.setHeader("Content-Type", "application/json");
    response.end(
      JSON.stringify({
        id: randomUUID(),
        type: "message",
        role: "assistant",
        model: body.model,
        content: [{ type: "text", text: "isolated reply" }],
        stop_reason: "end_turn",
        usage: { input_tokens: 5, output_tokens: 2 },
      }),
    );
  } catch {
    response.writeHead(400).end();
  }
});
let child, stopBridge, base;
const profile = {
  selected: { provider: "fixture", model: "old" },
  revision: 1,
  routes: {},
  keys: {},
  loseRestoreReply: false,
};
let accountUser = null;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function api(
  route,
  body,
  status = 200,
  method = body === undefined ? "GET" : "POST",
) {
  const r = await fetch(base + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20000),
  });
  const value = await r.json();
  assert.equal(r.status, status, `${route}: ${JSON.stringify(value)}`);
  return value;
}
try {
  if (!ownedSource) {
    await fs.mkdir(source, { mode: 0o700, recursive: true });
    const generation = randomUUID();
    const agent = "org.example.new-agent:desktop";
    const records = [
      { snapshot: generation, records: 1 },
      {
        fact: {
          kind: "session",
          value: {
            nativeId: "s",
            productIds: [agent],
            unknownClient: false,
            title: "New plugin fixture",
            partial: false,
            usage: {},
          },
        },
        conversation: [],
      },
      { commit: generation },
    ];
    if (!bundled) {
      await fs.writeFile(
        path.join(source, "facts.jsonl"),
        records.map((row) => JSON.stringify(row)).join("\n") + "\n",
        { mode: 0o600 },
      );
    }
    bridge.apply(
      {
        effect: (start) => {
          stopBridge = start();
        },
        logger: {
          warn: (message) => {
            throw Error(message);
          },
        },
        get: (service) =>
          service === "deepseekAccount"
            ? {
                getState: async () => ({
                  status: accountUser ? "credential-stored" : "signed-out",
                }),
                getDeviceIdentity: async () => ({ userId: accountUser }),
              }
            : {
                resolveCallConfig: async () => {},
                listModels: async (provider) => {
                  assert.equal(provider, "deepseek-account");
                  return [{ id: "account-model", name: "Account model" }];
                },
              },
        settingsController: {
          describe: () => ({
            writable: true,
            namespaces: [
              {
                ns: "llm-pi-ai",
                revision: profile.revision,
                value: { providers: profile.routes },
              },
            ],
          }),
          mutate: async (_, edits, revision) => {
            assert.equal(String(revision), String(profile.revision));
            for (const edit of edits) {
              const id = edit.path[1];
              if (edit.op === "set") profile.routes[id] = edit.value;
              else delete profile.routes[id];
            }
            profile.revision++;
          },
        },
        credentialsController: {
          set: async (ref, key) => {
            profile.keys[ref] = key;
          },
          unset: async (ref) => {
            delete profile.keys[ref];
            if (profile.loseRestoreReply) {
              profile.loseRestoreReply = false;
              throw Error("lost restore reply");
            }
          },
        },
        agentDefaultModel: {
          currentSelection: () => profile.selected,
          saveSelection: async (selection) => {
            profile.selected = selection;
          },
        },
      },
      { outputDirectory: source },
    );
  }
  for (let attempt = 0; attempt < 50; attempt++) {
    try {
      await fs.access(path.join(source, "agent-connection.key"));
      break;
    } catch {
      await sleep(100);
    }
  }
  await fs.access(path.join(source, "agent-connection.sock"));
  await fs.mkdir(home, { recursive: true });
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  child = spawn(binary, ["--data-dir", path.join(directory, "app")], {
    env: {
      ...process.env,
      TMPDIR: directory,
      LOCAL_AGENT_SOURCE_HOME: home,
      LOCAL_AGENT_SERVICE_TOKEN: token,
      LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
      LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let serviceError = "";
  child.stderr.on("data", (chunk) => {
    serviceError = (serviceError + chunk).slice(-2000);
  });
  const [line] = await Promise.race([
    once(createInterface({ input: child.stdout }), "line", {
      signal: AbortSignal.timeout(15000),
    }),
    once(child, "exit").then(() => {
      throw Error("Isolated service failed to start: " + serviceError);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  const pluginId =
    bundled || ownedSource
      ? "net.worksha.dsh-desktop"
      : "org.example.new-agent";
  const agentId = `${pluginId}:desktop`;
  const connect = (action, status) =>
    api("/api/agents/model-connection", { agentId, action }, status);
  if (bundled) {
    assert.deepEqual((await api("/api/plugin-sources")).items, []);
    await assert.rejects(fs.access(path.join(source, "facts.jsonl")), {
      code: "ENOENT",
    });
  } else {
    const folder = path.join(directory, "package");
    await fs.cp("sdk/plugins/examples/dsh-desktop/package", folder, {
      recursive: true,
    });
    const file = path.join(folder, "manifest.json");
    const manifest = JSON.parse(await fs.readFile(file, "utf8"));
    if (!ownedSource) {
      manifest.id = pluginId;
      manifest.publisher = "org.example";
      manifest.displayName = "New Agent fixture";
    }
    await fs.writeFile(file, JSON.stringify(manifest));
    const archive = path.join(directory, "source.laplug");
    const key = path.join(directory, "publisher.key");
    sdk("keygen", key);
    sdk("pack", folder, archive, key);
    const inspected = await api("/api/plugin-sources/inspect", {
      path: archive,
    });
    const pkg = { pluginId: manifest.id, digest: inspected.digest };
    await api("/api/plugin-sources/install", {
      packageRef: inspected.packageRef,
      digest: pkg.digest,
      approvePublisher: true,
      expectedSigningKey: inspected.signingKey,
    });
    await api("/api/plugin-sources/state", { package: pkg, action: "enable" });
    await api("/api/domain/source-authorize", {
      ...pkg,
      path: path.join(source, "facts.jsonl"),
    });
    const entry = (await api("/api/snapshot")).agents.find(
      (agent) => agent.id === agentId,
    );
    assert.equal(entry.connectionAdapter, "agent.connection@1");
    if (!ownedSource) {
      await api("/api/domain/source-refresh", pkg);
      const consent = await api("/api/domain/consent-review", pkg);
      await api("/api/domain/consent", {
        ...pkg,
        enabled: true,
        persistMetadata: false,
        reviewRef: consent.reviewRef,
      });
      const page = await api("/api/domain/sessions", {
        scope: { kind: "external", sourceIds: [agentId] },
        filter: { productId: agentId, query: null, days: null },
        snapshotToken: null,
        cursor: null,
      });
      assert.equal(page.items[0].record.title, "New plugin fixture");
    }
  }
  assert.equal(
    (await connect({ kind: "status" })).state,
    "authorization-required",
  );
  if (bundled) {
    assert.equal(
      (await connect({ kind: "account-models" }, 403)).code,
      "permission_denied",
    );
    const initial = await connect({ kind: "authorize" });
    assert.deepEqual(await connect({ kind: "account-models" }), {
      signedIn: false,
      models: [],
    });
    await connect({ kind: "preview-account", model: "account-model" }, 400);
    accountUser = "account-a";
    assert.deepEqual(await connect({ kind: "account-models" }), {
      signedIn: true,
      models: [{ id: "account-model", name: "Account model" }],
    });
    await connect({ kind: "preview-account", model: "unlisted" }, 409);
    const first = await connect({
      kind: "preview-account",
      model: "account-model",
    });
    assert.deepEqual(
      (await connect({ kind: "status" })).selected,
      initial.selected,
    );
    accountUser = "account-b";
    await connect({ kind: "apply", reviewId: first.reviewId }, 409);
    const preview = await connect({
      kind: "preview-account",
      model: "account-model",
    });
    const applied = await connect({
      kind: "apply",
      reviewId: preview.reviewId,
    });
    assert.equal(applied.selectedTarget, true);
    assert.deepEqual((await connect({ kind: "status" })).currentTarget, {
      kind: "account",
      name: "DeepSeek 官方账号",
      model: "account-model",
    });
    assert.deepEqual((await connect({ kind: "status" })).recovery, {
      kind: "account",
      reviewId: preview.reviewId,
      before: initial.selected,
    });
    assert.deepEqual(
      (await connect({ kind: "restore", reviewId: preview.reviewId })).selected,
      initial.selected,
    );
    assert.deepEqual(profile.routes, {});
    assert.deepEqual(profile.keys, {});
    assert.equal(traffic.length, 0);
    await connect({ kind: "revoke" });
  }
  const provider = {
    id: `custom-${randomUUID()}`,
    name: "Isolated upstream",
    vendor: "custom",
    protocol: "messages",
    endpoint: `http://127.0.0.1:${upstream.address().port}`,
    model: "test-model",
    models: ["test-model"],
    configured: true,
    credentialRef: null,
    credentialLabel: null,
    revision: 0,
    category: "local",
  };
  const target = { kind: "provider", id: provider.id, model: provider.model };
  await connect({ kind: "preview", target }, 403);
  const original = await connect({ kind: "authorize" });
  assert.equal(original.state, "authorized");
  await api("/api/providers", provider, 200, "PUT");
  assert.equal(
    (await connect({ kind: "preview", target }, 400)).code,
    "connection_unverified",
  );
  assert.equal(
    (
      await api(`/api/providers/${provider.id}/verify`, {
        expectedRevision: 1,
        model: provider.model,
      })
    ).state,
    "verified",
  );
  const preview = await connect({ kind: "preview", target });
  assert.equal(preview.scope, "new-sessions");
  assert.deepEqual(preview.before, original.selected);
  assert.deepEqual(
    (await connect({ kind: "status" })).selected,
    original.selected,
  );
  if (!ownedSource) {
    profile.revision++;
    const stale = await connect(
      { kind: "apply", reviewId: preview.reviewId },
      409,
    );
    assert.equal(stale.code, "stale_snapshot");
  }
  const reviewed = ownedSource
    ? preview
    : await connect({ kind: "preview", target });
  const applied = await connect({ kind: "apply", reviewId: reviewed.reviewId });
  assert.equal(applied.configured, true);
  assert.equal(applied.selectedTarget, true);
  assert.equal(applied.selected.model, "local-agent-default");
  const providerStatus = await connect({ kind: "status" });
  assert.deepEqual(providerStatus.currentTarget, {
    kind: "provider",
    name: provider.name,
    model: provider.model,
  });
  if (original.selected) {
    assert.deepEqual(providerStatus.recovery, {
      reviewId: reviewed.reviewId,
      before: original.selected,
    });
  }
  const check = await connect({ kind: "check", reviewId: reviewed.reviewId });
  assert.equal(check.selectedTarget, true);
  if (!ownedSource) {
    const installed = profile.routes[applied.selected.provider];
    const key = profile.keys[installed.apiKeyEnv];
    const call = () =>
      fetch(installed.baseURL + "/v1/messages", {
        method: "POST",
        headers: { "x-api-key": key, "content-type": "application/json" },
        body: JSON.stringify({
          model: applied.selected.model,
          max_tokens: 32,
          messages: [{ role: "user", content: "fixture" }],
        }),
      });
    const response = await call();
    assert.equal(response.status, 200, await response.clone().text());
    assert.equal((await response.json()).content[0].text, "isolated reply");
    assert.equal(traffic.length, 2);
    profile.loseRestoreReply = true;
    const uncertain = await connect(
      { kind: "restore", reviewId: reviewed.reviewId },
      400,
    );
    assert.equal(uncertain.code, "connection_result_unknown");
    assert.deepEqual(
      {
        ...profile.selected,
        reasoningEffort: profile.selected.reasoningEffort ?? null,
      },
      original.selected,
    );
    const recovered = await connect({
      kind: "check",
      reviewId: reviewed.reviewId,
    });
    assert.deepEqual(recovered.selected, original.selected);
    assert.equal(recovered.configured, false);
    assert.deepEqual(
      await connect({ kind: "check", reviewId: reviewed.reviewId }),
      recovered,
    );
    assert.equal((await connect({ kind: "status" })).currentTarget, undefined);
    assert.equal((await call()).status, 401);
  }
  const model = {
    id: randomUUID(),
    name: "Fixture service model",
    providerId: provider.id,
    model: provider.model,
    capabilities: {
      contextWindow: 96000,
      maxOutputTokens: 1024,
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
      name: "Isolated model service",
      defaultModelId: model.id,
      models: [model],
    },
    200,
    "PUT",
  );
  const serviceReview = await connect({
    kind: "preview",
    target: { kind: "service", id: service.id },
  });
  const serviceApply = await connect({
    kind: "apply",
    reviewId: serviceReview.reviewId,
  });
  assert.equal(serviceApply.configured, true);
  assert.equal(serviceApply.selectedTarget, true);
  assert.equal(serviceApply.selected.model, "local-agent-default");
  const serviceStatus = await connect({ kind: "status" });
  assert.deepEqual(serviceStatus.currentTarget, {
    kind: "service",
    name: service.name,
    model: model.name,
  });
  assert.equal(serviceStatus.recovery?.reviewId, serviceReview.reviewId);
  let dshGatewayRequest = false;
  if (ownedSource && process.env.DSH_TEST_TURN === "1") {
    const before = traffic.length;
    await fs.writeFile(
      path.join(source, "run-owned-turn"),
      serviceApply.selected.provider,
      { mode: 0o600, flag: "wx" },
    );
    let response;
    for (let i = 0; i < 100; i++) {
      try {
        const result = JSON.parse(
          await fs.readFile(
            path.join(source, "owned-turn-result.json"),
            "utf8",
          ),
        );
        assert.equal(result.accepted, true, result.code);
        const requests = await api("/api/requests");
        const rows = Array.isArray(requests)
          ? requests
          : (requests.items ?? requests.requests ?? []);
        response = rows.find(
          (row) =>
            row.agentProductId === agentId &&
            row.routing?.service?.serviceId === service.id &&
            row.httpStatus === 200 &&
            row.endedAt,
        );
        if (response) break;
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
      await sleep(250);
    }
    assert(
      response,
      "Real DSH runtime did not produce a verified gateway request",
    );
    assert(traffic.length > before);
    dshGatewayRequest = true;
  }
  if (ownedSource && serviceReview.before) {
    const restore = await connect({
      kind: "restore",
      reviewId: serviceReview.reviewId,
    });
    assert.deepEqual(restore.selected, serviceReview.before);
  }
  assert.equal(
    (await connect({ kind: "revoke" })).state,
    "authorization-required",
  );
  assert.equal(
    (await connect({ kind: "preview", target }, 403)).code,
    "permission_denied",
  );
  if (!ownedSource) {
    const installed = profile.routes[serviceApply.selected.provider];
    const key = profile.keys[installed.apiKeyEnv];
    await connect({ kind: "authorize" });
    assert.equal(
      (
        await fetch(installed.baseURL + "/v1/models", {
          headers: { "x-api-key": key },
        })
      ).status,
      401,
    );
    await api(`/api/model-services/${service.id}/delete`, {
      expectedRevision: service.revision,
    });
  }
  console.log(
    JSON.stringify({
      agentId,
      bundledConnectionWithoutSource: bundled,
      deepSeekAccountSwitchAndRestore: bundled,
      installedDshRuntime: !!ownedSource,
      providerApplied: true,
      modelServiceApplied: true,
      nativeReadback: true,
      authorizationSeparated: true,
      stalePreviewRejected: !ownedSource,
      mockGatewayRequestAndRestore: !ownedSource,
      recoveredLostRestoreReply: !ownedSource,
      dshGatewayRequest,
      liveModelRequests: 0,
      keychainCalls: 0,
    }),
  );
} finally {
  if (child?.exitCode === null) {
    const done = once(child, "exit");
    if (base)
      await api("/api/runtime/shutdown", {}).catch(() => child.kill("SIGTERM"));
    else child.kill("SIGTERM");
    await done;
  }
  stopBridge?.();
  upstream.closeAllConnections();
  if (upstream.listening)
    await new Promise((resolve) => upstream.close(resolve));
  await fs.rm(directory, { recursive: true, force: true });
}
