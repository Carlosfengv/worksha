import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  rmSync,
  readdirSync,
  statSync,
  realpathSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";
import http from "node:http";

const directory = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-providers-")),
);
const home = path.join(directory, "home");
mkdirSync(path.join(home, ".claude"), { recursive: true });
mkdirSync(path.join(home, ".codex"), { recursive: true });
const originalClaude =
  JSON.stringify(
    {
      env: {
        ANTHROPIC_AUTH_TOKEN: "original-config-secret-sentinel",
        KEEP: "safe",
      },
      permissions: { deny: ["Read(.env)"] },
      mcpServers: { safe: { command: "safe-server" } },
      unknown: true,
    },
    null,
    2,
  ) + "\n";
const originalCodex =
  "# preferences retained\nmodel='before'\n[mcp_servers.safe]\ncommand='safe-server'\n";
const claudePath = path.join(home, ".claude/settings.json"),
  codexPath = path.join(home, ".codex/config.toml");
writeFileSync(claudePath, originalClaude);
writeFileSync(codexPath, originalCodex);
writeFileSync(
  path.join(home, ".codex/auth.json"),
  "unreadable-native-auth-sentinel",
);
const token = randomUUID();
const secret = "provider-key-sentinel-isolated";
let upstreamBehavior = "ok",
  requests = [],
  stderr = "",
  child;
const upstream = http.createServer((req, res) => {
  let text = "";
  req.on("data", (chunk) => (text += chunk));
  req.on("end", () => {
    if (req.method === "GET" && req.url === "/v1/models") {
      assert.equal(req.headers.authorization, `Bearer ${secret}`);
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(
        JSON.stringify({
          data: [{ id: "catalog-a", name: "Catalog A" }, { id: "catalog-b" }],
        }),
      );
      return;
    }
    const body = JSON.parse(text);
    requests.push({
      url: req.url,
      auth: req.headers.authorization,
      apiKey: req.headers["x-api-key"],
      body,
    });
    if (upstreamBehavior === "unauthorized") {
      res.writeHead(401);
      res.end(JSON.stringify({ error: secret }));
      return;
    }
    if (upstreamBehavior === "redirect") {
      res.writeHead(307, { Location: "https://must-not-follow.invalid" });
      res.end();
      return;
    }
    if (upstreamBehavior === "wrong") {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ choices: [] }));
      return;
    }
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(
      JSON.stringify(
        req.url.endsWith("/messages")
          ? {
              id: "msg-fixture",
              type: "message",
              role: "assistant",
              model: body.model,
              content: [{ type: "text", text: "probe-response-sentinel" }],
              usage: { input_tokens: 1, output_tokens: 1 },
            }
          : {
              id: "resp-fixture",
              object: "response",
              model: body.model,
              status: "completed",
              output: [
                {
                  type: "message",
                  content: [
                    { type: "output_text", text: "probe-response-sentinel" },
                  ],
                },
              ],
              usage: { input_tokens: 1, output_tokens: 1 },
            },
      ),
    );
  });
});
await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
const endpoint = `http://127.0.0.1:${upstream.address().port}`;
try {
  child = spawn(
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? ".work/provider-tests-target/debug/local-agent-service",
    ["--data-dir", path.join(directory, "app")],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SERVICE_TOKEN: token,
        LOCAL_AGENT_SOURCE_HOME: home,
        LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
        LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
        LOCAL_AGENT_IMPORT_FIXTURE_KEY: "isolated-import-env-key",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  child.stderr.on("data", (b) => (stderr += b));
  const line = await Promise.race([
    once(createInterface({ input: child.stdout }), "line").then(([line]) =>
      JSON.parse(line),
    ),
    once(child, "exit").then(() => {
      throw Error("Provider service exited before ready");
    }),
    new Promise((_, reject) => {
      const t = setTimeout(
        () => reject(Error("Service readiness timeout")),
        8000,
      );
      t.unref();
    }),
  ]);
  const base = `http://127.0.0.1:${line.port}`;
  const request = async (
    route,
    body,
    expected = 200,
    method = body === undefined ? "GET" : "POST",
  ) => {
    const response = await fetch(base + route, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const json = await response.json();
    assert.equal(
      response.status,
      expected,
      `Status ${response.status} on ${route}: ${JSON.stringify(json)}`,
    );
    assert(!JSON.stringify(json).includes(secret));
    assert(!JSON.stringify(json).includes("original-config-secret-sentinel"));
    return json;
  };
  assert.equal((await fetch(base + "/api/provider-status")).status, 401);
  assert.equal(
    (
      await fetch(base + "/api/provider-credentials", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          Origin: "https://untrusted.invalid",
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ label: "x", secret }),
      })
    ).status,
    401,
  );
  let status = await request("/api/provider-status");
  assert(
    status.runtimes.every((r) => r.reason === "隔离临时目录测试进程状态"),
    "Test requires the isolated in-memory vault service; refusing to create credentials",
  );
  assert.equal(status.configs.length, 5);
  assert.equal(status.configs.find((c) => c.agentId === "antigravity").state,"unsupported");
  assert.equal(status.transactions.length, 0);
  assert(
    status.configs
      .filter((c) => !["claude-desktop", "antigravity"].includes(c.agentId))
      .every((c) => c.revision),
  );
  assert(status.configs.find((c) => c.agentId === "claude-desktop").error);
  const credential = await request("/api/provider-credentials", {
    label: "isolated-key",
    secret,
  });
  let catalog = (await request("/api/snapshot")).providers;
  let provider = {
    ...catalog.find((p) => p.id === "openai"),
    name: "Isolated provider",
    endpoint: endpoint + "/v1",
    model: "fixture-model",
    models: ["model-a", "model-b"],
    category: "local",
    ...credential,
  };
  const save = async (p, expected = 200) => {
    const response = await fetch(base + "/api/providers", {
      method: "PUT",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(p),
    });
    assert.equal(response.status, expected);
    return response.json();
  };
  for (const vendor of ["ollama", "vllm"]) {
    const connection = {
      ...catalog.find((p) => p.id === vendor),
      id: `custom-${randomUUID()}`,
      vendor,
      name: `${vendor} workstation`,
      endpoint: endpoint + "/v1",
      model: "fixture-model",
    };
    await save(connection);
    const saved = (await request("/api/snapshot")).providers.find(
      (p) => p.id === connection.id,
    );
    assert.equal(saved.vendor, vendor);
    assert.equal(saved.endpoint, connection.endpoint);
    assert.equal(saved.name, connection.name);
    assert.equal(saved.credentialRef, null);
    assert.equal(
      (await request("/api/snapshot")).providers.find((p) => p.id === vendor)
        .configured,
      false,
    );
  }
  await save(provider);
  provider = (await request("/api/snapshot")).providers.find(
    (p) => p.id === provider.id,
  );
  await request(
    "/api/provider-switches",
    {
      agentId: "codex",
      providerId: provider.id,
      expectedRevision: status.configs.find((c) => c.agentId === "codex")
        .revision,
    },
    400,
  );
  assert.equal(requests.length, 0, "saving metadata must not probe upstream");
  assert.equal(
    (await fetch(base + "/api/provider-models", { method: "POST" })).status,
    401,
  );
  const beforeModels = JSON.stringify(
    (await request("/api/snapshot")).providers,
  );
  const discoveredModels = await request("/api/provider-models", {
    endpoint: provider.endpoint,
    protocol: provider.protocol,
    vendor: provider.vendor ?? "custom",
    credentialRef: provider.credentialRef,
  });
  assert.deepEqual(
    discoveredModels.models.map((m) => m.id),
    ["catalog-a", "catalog-b"],
  );
  assert.equal(discoveredModels.sourceUrl, endpoint + "/v1/models");
  assert.equal(
    JSON.stringify((await request("/api/snapshot")).providers),
    beforeModels,
    "discovery is read-only until explicitly saved",
  );
  assert.equal(readFileSync(claudePath, "utf8"), originalClaude);
  assert.equal(readFileSync(codexPath, "utf8"), originalCodex);
  let verified = await request(`/api/providers/${provider.id}/verify`, {
    expectedRevision: provider.revision,
  });
  assert.equal(verified.state, "verified");
  assert.equal(requests.at(-1).auth, `Bearer ${secret}`);
  assert.equal(requests.at(-1).url, "/v1/responses");
  assert.equal(requests.at(-1).body.max_output_tokens, 16);
  const savedBeforeModelSwitch = JSON.stringify(
    (await request("/api/snapshot")).providers,
  );
  const modelDraft = {
    agentId: "codex",
    providerId: provider.id,
    expectedProviderRevision: provider.revision,
    expectedRevision: status.configs.find((c) => c.agentId === "codex")
      .revision,
    model: "model-b",
  };
  const beforeInvalid = requests.length;
  await request("/api/provider-switches", modelDraft, 400);
  await request(
    `/api/providers/${provider.id}/verify`,
    { expectedRevision: provider.revision, model: "not-in-catalog" },
    400,
  );
  await request(
    "/api/provider-switches",
    { ...modelDraft, model: "not-in-catalog" },
    400,
  );
  await request(
    "/api/provider-switches",
    { ...modelDraft, providerId: null, expectedProviderRevision: undefined },
    400,
  );
  assert.equal(
    requests.length,
    beforeInvalid,
    "invalid model must not reach upstream",
  );
  assert.equal((await request("/api/provider-status")).transactions.length, 0);
  const selectedVerification = await request(
    `/api/providers/${provider.id}/verify`,
    { expectedRevision: provider.revision, model: "model-b" },
  );
  assert.equal(selectedVerification.model, "model-b");
  assert.equal(selectedVerification.state, "verified");
  assert.equal(requests.at(-1).body.model, "model-b");
  const byModel = (await request("/api/provider-status")).verifications.filter(
    (v) => v.providerId === provider.id,
  );
  assert.equal(byModel.length, 2);
  assert(byModel.every((v) => v.state === "verified"));
  // Codex publishes the whole enabled group, including models other than the selected default.
  for (const model of provider.models.filter(m => !byModel.some(v => v.model === m) && !(provider.disabledModels ?? []).includes(m))) {
    await request(`/api/providers/${provider.id}/verify`, {expectedRevision:provider.revision,model});
  }
  const selectedSwitch = await request("/api/provider-switches", modelDraft);
  assert.equal(selectedSwitch.model, "model-b");
  assert.equal(selectedSwitch.provider.model, "fixture-model");
  assert(selectedSwitch.targetName.includes("model-b"));
  assert.equal(readFileSync(codexPath, "utf8"), originalCodex);
  assert.equal(
    readFileSync(selectedSwitch.backupPath).subarray(0, 4).toString(),
    "LAC2",
  );
  await request(`/api/provider-switches/${selectedSwitch.id}/apply`, {});
  assert.match(
    readFileSync(codexPath, "utf8"),
    /^model\s*=\s*["']model-b["']\s*$/m,
  );
  assert.equal(
    (await request("/api/provider-status")).configs.find(
      (c) => c.agentId === "codex",
    ).model,
    "model-b",
  );
  const selectedRestore = await request(
    `/api/provider-switches/${selectedSwitch.id}/restore`,
    {},
  );
  await request(`/api/provider-switches/${selectedRestore.id}/apply`, {});
  assert.equal(readFileSync(codexPath, "utf8"), originalCodex);
  assert.equal(
    JSON.stringify((await request("/api/snapshot")).providers),
    savedBeforeModelSwitch,
    "Agent selection must not change Provider defaults or catalog",
  );
  const availabilityPending = await request(
    "/api/provider-switches",
    modelDraft,
  );
  const availabilityRevision = provider;
  await save({ ...provider, disabledModels: ["model-b"] });
  provider = (await request("/api/snapshot")).providers.find(
    (p) => p.id === provider.id,
  );
  assert.deepEqual(provider.disabledModels, ["model-b"]);
  assert(
    provider.models.includes("model-b"),
    "disabling must retain the catalog",
  );
  const beforeDisabled = requests.length;
  await request(
    `/api/providers/${provider.id}/verify`,
    {
      expectedRevision: provider.revision,
      model: "model-b",
    },
    400,
  );
  await request(
    "/api/provider-switches",
    {
      ...modelDraft,
      expectedProviderRevision: provider.revision,
    },
    400,
  );
  await request(
    `/api/provider-switches/${availabilityPending.id}/apply`,
    {},
    400,
  );
  assert.equal(
    requests.length,
    beforeDisabled,
    "disabled model must not reach upstream",
  );
  assert.equal(readFileSync(codexPath, "utf8"), originalCodex);
  await request(`/api/provider-switches/${availabilityPending.id}/cancel`, {});
  await save(availabilityRevision, 400);
  await save({ ...provider, disabledModels: [...provider.models] });
  provider = (await request("/api/snapshot")).providers.find(
    (p) => p.id === provider.id,
  );
  await request(
    `/api/providers/${provider.id}/verify`,
    { expectedRevision: provider.revision },
    400,
  );
  await request(
    "/api/provider-switches",
    {
      ...modelDraft,
      model: undefined,
      expectedProviderRevision: provider.revision,
    },
    400,
  );
  assert.equal(
    requests.length,
    beforeDisabled,
    "disabled default cannot bypass selection",
  );
  await request(
    "/api/observation/bindings",
    {
      agentId: "codex",
      providerId: provider.id,
      expectedRevision: 0,
      enabled: true,
    },
    400,
    "PUT",
  );
  await save({ ...provider, models: ["new-catalog-model"] });
  provider = (await request("/api/snapshot")).providers.find(
    (p) => p.id === provider.id,
  );
  assert(
    provider.disabledModels.includes("model-b"),
    "catalog refresh preserves disabled status",
  );
  assert(provider.models.includes("model-b"));
  assert(!provider.disabledModels.includes("new-catalog-model"));
  await save({ ...provider, disabledModels: ["unknown-model"] }, 400);
  await save({ ...provider, disabledModels: [] });
  provider = (await request("/api/snapshot")).providers.find(
    (p) => p.id === provider.id,
  );
  await request(`/api/providers/${provider.id}/verify`, {
    expectedRevision: provider.revision,
  });
  for (const model of provider.models.filter(m => m !== provider.model && !(provider.disabledModels ?? []).includes(m))) {
    await request(`/api/providers/${provider.id}/verify`, { expectedRevision:provider.revision, model });
  }
  let t = await request("/api/provider-switches", {
    agentId: "codex",
    providerId: provider.id,
    expectedRevision: status.configs.find((c) => c.agentId === "codex")
      .revision,
  });
  assert.equal(readFileSync(codexPath, "utf8"), originalCodex);
  assert.equal(t.state, "pending");
  assert.equal(statSync(t.backupPath).mode & 0o777, 0o600);
  const deferred = await request(`/api/provider-switches/${t.id}/defer`, {});
  assert.equal(deferred.state, "pending");
  assert.equal(deferred.activation.state, "deferred");
  assert.equal(readFileSync(codexPath, "utf8"), originalCodex);
  assert.equal((await request(`/api/provider-switches/${t.id}/check`, {})).id, t.id);
  await request(`/api/provider-switches/${t.id}/launched`, {}, 409);
  await request(`/api/provider-switches/${t.id}/apply`, {});
  assert.equal((await request(`/api/provider-switches/${t.id}/launched`, {})).activation.state, "launch_requested");
  assert(readFileSync(codexPath, "utf8").includes("[mcp_servers.safe]"));
  assert(readFileSync(codexPath, "utf8").includes("auth"));
  assert(!readFileSync(codexPath, "utf8").includes(secret));
  const restore = await request(`/api/provider-switches/${t.id}/restore`, {});
  await request(`/api/provider-switches/${restore.id}/apply`, {});
  assert.equal(readFileSync(codexPath, "utf8"), originalCodex);
  status = await request("/api/provider-status");
  t = await request("/api/provider-switches", {
    agentId: "codex",
    providerId: provider.id,
    expectedRevision: status.configs.find((c) => c.agentId === "codex")
      .revision,
  });
  writeFileSync(codexPath, originalCodex + "# external edit\n");
  await request(`/api/provider-switches/${t.id}/check`, {}, 409);
  await request(`/api/provider-switches/${t.id}/apply`, {}, 409);
  assert(readFileSync(codexPath, "utf8").endsWith("# external edit\n"));
  await request(`/api/provider-switches/${t.id}/cancel`, {});
  await save({ ...provider, model: "new-default", models: [] });
  await save(provider, 400);
  provider = (await request("/api/snapshot")).providers.find(
    (p) => p.id === provider.id,
  );
  assert(
    provider.models.includes("model-a") && provider.models.includes("model-b"),
  );
  status = await request("/api/provider-status");
  assert.equal(
    status.verifications.find((v) => v.providerId === provider.id).state,
    "outdated",
  );
  for (const behavior of ["wrong", "unauthorized", "redirect"]) {
    upstreamBehavior = behavior;
    verified = await request(`/api/providers/${provider.id}/verify`, {
      expectedRevision: provider.revision,
    });
    assert.equal(verified.state, "failed");
    assert(!verified.message.includes(secret));
  }
  upstreamBehavior = "ok";
  await request(`/api/providers/${provider.id}/verify`, {
    expectedRevision: provider.revision,
  });
  const claudeProvider = {
    ...catalog.find((p) => p.id === "anthropic"),
    endpoint,
    category: "local",
    protocol: "messages",
    model: "claude-fixture",
    ...credential,
  };
  await save(claudeProvider);
  const claude = (await request("/api/snapshot")).providers.find(
    (p) => p.id === "anthropic",
  );
  await request(`/api/providers/${claude.id}/verify`, {
    expectedRevision: claude.revision,
  });
  assert.equal(requests.at(-1).apiKey, secret);
  assert.equal(requests.at(-1).url, "/v1/messages");
  status = await request("/api/provider-status");
  t = await request("/api/provider-switches", {
    agentId: "claude",
    providerId: claude.id,
    expectedRevision: status.configs.find((c) => c.agentId === "claude")
      .revision,
  });
  assert.equal(t.routing.clientProtocol, "messages");
  assert.equal(t.routing.upstreamProtocol, "messages");
  assert.notEqual(t.routing.localEndpoint, claude.endpoint);
  assert.equal(readFileSync(claudePath, "utf8"), originalClaude);
  await request(`/api/provider-switches/${t.id}/apply`, {});
  const changed = JSON.parse(readFileSync(claudePath, "utf8"));
  assert.equal(changed.env.ANTHROPIC_BASE_URL, t.routing.localEndpoint);
  assert.equal(changed.env.KEEP, "safe");
  assert.equal(changed.permissions.deny[0], "Read(.env)");
  assert(changed.apiKeyHelper.includes("--credential"));
  let r = await request(`/api/provider-switches/${t.id}/restore`, {});
  await request(`/api/provider-switches/${r.id}/apply`, {});
  assert.equal(readFileSync(claudePath, "utf8"), originalClaude);
  const adapted = await request("/api/provider-switches", {
    agentId: "claude",
    providerId: provider.id,
    expectedRevision: (await request("/api/provider-status")).configs.find(
      (c) => c.agentId === "claude",
    ).revision,
  });
  assert.equal(adapted.routing.clientProtocol, "messages");
  assert.equal(adapted.routing.upstreamProtocol, "responses");
  assert.equal(readFileSync(claudePath, "utf8"), originalClaude);
  await request(`/api/provider-switches/${adapted.id}/cancel`, {});
  // Publishing a Provider group requires independent verification of every enabled model.
  for (const agentId of ["opencode","claude-desktop","codex"]) {
  let nativePath=path.join(home,".config/opencode/opencode.json");
  const originalDesktop=JSON.stringify({inferenceProvider:"gateway",inferenceGatewayBaseUrl:"http://127.0.0.1:9/original",managedMcpServers:[{name:"keep"}]});
  if(agentId==="claude-desktop") {
    const profileRoot=path.join(home,"Library/Application Support/Claude-3p/configLibrary");
    mkdirSync(profileRoot,{recursive:true});
    const profileId=randomUUID();
    writeFileSync(path.join(profileRoot,"_meta.json"),JSON.stringify({appliedId:profileId}));
    nativePath=path.join(profileRoot,profileId+".json");
    writeFileSync(nativePath,originalDesktop);
  }
  if(agentId==="codex") nativePath=codexPath;
  const originalGroupConfig=existsSync(nativePath)?readFileSync(nativePath,"utf8"):null;
  const groupId=`custom-${randomUUID()}`;
  await save({...provider,id:groupId,revision:0,model:"group-a",models:["group-a","group-b","disabled"],disabledModels:["disabled"]});
  const groupProvider=(await request("/api/snapshot")).providers.find(p=>p.id===groupId);
  const groupDraft={agentId,providerId:groupId,expectedProviderRevision:groupProvider.revision,model:"group-a",expectedRevision:(await request("/api/provider-status")).configs.find(c=>c.agentId===agentId).revision};
  await request(`/api/providers/${groupId}/verify`,{expectedRevision:groupProvider.revision,model:"group-a"});
  await request("/api/provider-switches",groupDraft,400);
  await request(`/api/providers/${groupId}/verify`,{expectedRevision:groupProvider.revision,model:"group-b"});
  const groupTransaction=await request("/api/provider-switches",groupDraft);
  assert.deepEqual(groupTransaction.routing.models,["group-a","group-b"]);
  await request(`/api/provider-switches/${groupTransaction.id}/apply`,{});
  if(agentId==="codex") {
    assert(readFileSync(nativePath,"utf8").includes("model_catalog_url"));
    const catalog=await fetch(groupTransaction.routing.localEndpoint+"/codex/models.json?client_version=0.163.0",{headers:{Authorization:`Bearer ${requests.at(-1).auth?.slice(7)??""}`}});
    assert.equal(catalog.status,401,"upstream credentials cannot authenticate the local catalog");
  } else {
  const groupedConfig=JSON.parse(readFileSync(nativePath,"utf8"));
  if(agentId==="opencode") {
    const groupKey=groupedConfig.model.split("/")[0];
    assert.deepEqual(Object.keys(groupedConfig.provider[groupKey].models),["group-a","group-b"]);
  } else {
    assert.equal(groupedConfig.inferenceModels.length,2);
    assert.notEqual(groupedConfig.inferenceModels[0].name,groupedConfig.inferenceModels[1].name);
    assert(groupedConfig.inferenceModels.every(m=>m.name.startsWith("claude-sonnet-5-local-agent-")));
    assert.deepEqual(groupedConfig.inferenceModels.map(m=>m.labelOverride.split(" · ").at(-1)),["group-a","group-b"]);
    assert.deepEqual(groupedConfig.managedMcpServers,[{name:"keep"}]);
  }
  }
  const groupRestore=await request(`/api/provider-switches/${groupTransaction.id}/restore`,{});
  await request(`/api/provider-switches/${groupRestore.id}/apply`,{});
  if(agentId==="opencode") assert(!existsSync(nativePath),"restoring previously absent OpenCode config must remove only the generated config");
  else assert.equal(readFileSync(nativePath,"utf8"),originalGroupConfig,"restore must recover the exact shared config or desktop Profile");
  }
  let custom = {
    ...provider,
    id: `custom-${randomUUID()}`,
    revision: 0,
    name: "Local fixture",
  };
  await save(custom);
  assert(
    (await request("/api/snapshot")).providers.some((p) => p.id === custom.id),
  );
  // Real HTTP import flow, with only disposable native sources and MemoryVault.
  const beforeImportCount = (await request("/api/snapshot")).providers.length;
  const importClaude = JSON.stringify({
    env: {
      ANTHROPIC_BASE_URL: endpoint,
      ANTHROPIC_AUTH_TOKEN: "original-config-secret-sentinel",
      ANTHROPIC_MODEL: "imported-claude-model",
    },
  });
  writeFileSync(claudePath, importClaude);
  writeFileSync(
    codexPath,
    `model='imported-codex-model'\nmodel_provider='import_proxy'\n[model_providers.import_proxy]\nbase_url='${endpoint}'\nwire_api='responses'\nenv_key='LOCAL_AGENT_IMPORT_FIXTURE_KEY'\n`,
  );
  let importCandidates = await request("/api/provider-imports");
  let importCandidate = importCandidates.find((c) => c.agentId === "claude");
  assert(importCandidate.canImport);
  assert.equal(importCandidate.model, "imported-claude-model");
  const importedClaude = await request("/api/provider-imports", {
    candidateId: importCandidate.id,
    expectedRevision: importCandidate.revision,
    model: null,
  });
  assert.equal(importedClaude.reused, false);
  assert.equal(importedClaude.provider.configured, true);
  assert.equal(importedClaude.provider.revision, 1);
  assert.equal(readFileSync(claudePath, "utf8"), importClaude);
  const reused = await request("/api/provider-imports", {
    candidateId: importCandidate.id,
    expectedRevision: importCandidate.revision,
    model: null,
  });
  assert.equal(reused.reused, true);
  assert.equal(reused.provider.id, importedClaude.provider.id);
  assert.equal(
    (await request("/api/snapshot")).providers.length,
    beforeImportCount + 1,
  );
  assert.equal(
    (
      await request(`/api/providers/${importedClaude.provider.id}/verify`, {
        expectedRevision: 1,
      })
    ).state,
    "verified",
  );
  assert.equal(requests.at(-1).apiKey, "original-config-secret-sentinel");
  const importedPreview = await request("/api/provider-switches", {
    agentId: "claude",
    providerId: importedClaude.provider.id,
    expectedRevision: (await request("/api/provider-status")).configs.find(
      (c) => c.agentId === "claude",
    ).revision,
  });
  assert.equal(importedPreview.provider.model, "imported-claude-model");
  await request(`/api/provider-switches/${importedPreview.id}/cancel`, {});
  importCandidate = importCandidates.find(
    (c) => c.sourceLabel === "import_proxy",
  );
  assert(importCandidate.canImport);
  const importedCodex = await request("/api/provider-imports", {
    candidateId: importCandidate.id,
    expectedRevision: importCandidate.revision,
    model: null,
  });
  assert.equal(importedCodex.provider.model, "imported-codex-model");
  assert.equal(
    (
      await request(`/api/providers/${importedCodex.provider.id}/verify`, {
        expectedRevision: 1,
      })
    ).state,
    "verified",
  );
  assert.equal(requests.at(-1).auth, "Bearer isolated-import-env-key");
  importCandidates = await request("/api/provider-imports");
  assert(
    importCandidates.some(
      (c) => c.existingProviderId === importedClaude.provider.id,
    ),
  );
  assert(
    importCandidates.some(
      (c) => c.existingProviderId === importedCodex.provider.id,
    ),
  );
  writeFileSync(claudePath, importClaude + "\n");
  importCandidate = importCandidates.find((c) => c.agentId === "claude");
  assert.equal(
    (
      await request(
        "/api/provider-imports",
        {
          candidateId: importCandidate.id,
          expectedRevision: importCandidate.revision,
          model: null,
        },
        409,
      )
    ).code,
    "conflict",
  );
  assert.equal(
    (
      await request(
        "/api/provider-imports",
        { candidateId: "../../outside", expectedRevision: "x", model: "m" },
        404,
      )
    ).code,
    "not_found",
  );
  for (const init of [
    {},
    {
      headers: {
        Authorization: `Bearer ${token}`,
        Origin: "https://example.invalid",
      },
    },
  ]) {
    assert.equal(
      (await fetch(base + "/api/provider-imports", init)).status,
      401,
    );
  }
  writeFileSync(claudePath, originalClaude);
  writeFileSync(codexPath, originalCodex);
  const walk = (dir) =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
      e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)],
    );
  for (const file of walk(path.join(directory, "app"))) {
    const bytes = readFileSync(file);
    assert(
      !bytes.includes(Buffer.from(secret)),
      `Key persisted in ${path.basename(file)}`,
    );
    assert(!bytes.includes(Buffer.from("original-config-secret-sentinel")));
    assert(!bytes.includes(Buffer.from("probe-response-sentinel")));
    assert(!bytes.includes(Buffer.from("isolated-import-env-key")));
  }
  assert.equal(
    readFileSync(path.join(home, ".codex/auth.json"), "utf8"),
    "unreadable-native-auth-sentinel",
  );
  assert.equal(stderr, "");
  console.log(
    "通过：原生配置事务、协议验证、加密备份和恢复回归；新增 Claude / Codex 导入、环境 Key、去重、源版本冲突、鉴权 / Origin 拒绝，以及导入 → 验证 → 切换预览。无密钥 / 正文额外落盘，仅使用临时来源、内存凭据库与本机测试服务。",
  );
} finally {
  if (child && child.exitCode === null) {
    const exit = once(child, "exit");
    child.kill();
    await exit;
  }
  await new Promise((resolve) => upstream.close(resolve));
  rmSync(directory, { recursive: true, force: true });
}
