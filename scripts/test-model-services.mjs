import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";
import http from "node:http";

const directory = realpathSync(mkdtempSync(path.join(tmpdir(), "local-agent-model-services-")));
const home = path.join(directory, "home");
mkdirSync(path.join(home, ".claude"), { recursive: true });
mkdirSync(path.join(home, ".config/opencode"), { recursive: true });
mkdirSync(path.join(home, ".codex"), { recursive: true });
const desktop = path.join(home, "Library/Application Support/Claude-3p/configLibrary");
const desktopProfile = randomUUID();
mkdirSync(desktop, { recursive: true });
writeFileSync(path.join(desktop, "_meta.json"), JSON.stringify({ appliedId: desktopProfile }), { mode: 0o600 });
const originals = {
  claude: JSON.stringify({ permissions: { allow: ["Read"] }, enabledPlugins: { keep: true }, env: { KEEP_ME: "yes" } }) + "\n",
  opencode: JSON.stringify({ provider: { untouched: { npm: "@ai-sdk/anthropic" } }, mcp: { keep: { type: "local", command: ["fixture"] } }, keep: true }) + "\n",
  "claude-desktop": JSON.stringify({ coworkEgressAllowedHosts: ["example.invalid"] }) + "\n",
  codex: "# preserved\nmodel = 'original'\n[mcp_servers.keep]\ncommand = 'fixture'\n",
};
const files = { claude: path.join(home, ".claude/settings.json"), opencode: path.join(home, ".config/opencode/opencode.json"), "claude-desktop": path.join(desktop, desktopProfile + ".json"), codex: path.join(home, ".codex/config.toml") };
for (const agent of Object.keys(files)) writeFileSync(files[agent], originals[agent], { mode: 0o600 });
const hash = file => createHash("sha256").update(readFileSync(file)).digest("hex");
const servers = [], seen = [];
let child, stderr = "", hold = false, arrive, release, unexpectedTool = false;
const token = randomUUID();
try {
  for (const protocol of ["messages", "chat-completions"]) {
    const server = http.createServer(async (req, res) => {
      let raw = ""; for await (const chunk of req) raw += chunk;
      const body = JSON.parse(raw); seen.push({ protocol, model: body.model, thinking: body.thinking?.type, tools: body.tools, toolChoice: body.tool_choice });
      if (hold) { hold = false; arrive(); await new Promise(resolve => { release = resolve; }); }
      if (unexpectedTool) {
        const tool = { type: "tool_use", id: "unexpected-one", name: "read", input: {} };
        if (body.stream) {
          const events = protocol === "messages"
            ? [{ type: "message_start", message: { id: "unexpected", type: "message", role: "assistant", model: body.model, content: [], usage: { input_tokens: 5, output_tokens: 0 } } }, { type: "content_block_start", index: 0, content_block: tool }, { type: "content_block_stop", index: 0 }, { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 2 } }, { type: "message_stop" }]
            : [{ id: "unexpected", model: body.model, choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: tool.id, type: "function", function: { name: "read", arguments: "{}" } }] }, finish_reason: "tool_calls" }] }];
          res.writeHead(200, { "Content-Type": "text/event-stream" }); res.end(events.map(event => `data: ${JSON.stringify(event)}\n\n`).join("") + (protocol === "messages" ? "" : "data: [DONE]\n\n"));
        } else {
          const result = protocol === "messages"
            ? { id: "unexpected", type: "message", role: "assistant", model: body.model, content: [tool], stop_reason: "tool_use", usage: { input_tokens: 5, output_tokens: 2 } }
            : { id: "unexpected", object: "chat.completion", model: body.model, choices: [{ index: 0, message: { role: "assistant", content: null, tool_calls: [{ id: tool.id, type: "function", function: { name: "read", arguments: "{}" } }] }, finish_reason: "tool_calls" }], usage: { prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 } };
          res.writeHead(200, { "Content-Type": "application/json" }); res.end(JSON.stringify(result));
        }
        return;
      }
      const result = protocol === "messages"
        ? { id: randomUUID(), type: "message", role: "assistant", model: body.model, content: [{ type: "text", text: "OK" }], stop_reason: "end_turn", usage: { input_tokens: 5, output_tokens: 2 } }
        : { id: randomUUID(), object: "chat.completion", model: body.model, choices: [{ index: 0, message: { role: "assistant", content: "OK" }, finish_reason: "stop" }], usage: { prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 } };
      res.writeHead(200, { "Content-Type": "application/json" }); res.end(JSON.stringify(result));
    });
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve)); servers.push(server);
  }
  child = spawn(process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service", ["--data-dir", path.join(directory, "app")], { env: { ...process.env, LOCAL_AGENT_SERVICE_TOKEN: token, LOCAL_AGENT_SOURCE_HOME: home, LOCAL_AGENT_TEST_MEMORY_VAULT: "1", LOCAL_AGENT_TEST_IDLE_AGENTS: "1" }, stdio: ["ignore", "pipe", "pipe"] });
  child.stderr.on("data", bytes => { stderr += bytes; });
  const ready = await Promise.race([once(createInterface({ input: child.stdout }), "line").then(([line]) => JSON.parse(line)), once(child, "exit").then(() => { throw Error("service exited: " + stderr); })]);
  const base = `http://127.0.0.1:${ready.port}`;
  const api = async (route, body, expected = 200, method = body === undefined ? "GET" : "POST") => {
    const r = await fetch(base + route, { method, headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(20000) });
    const text = await r.text(); assert.equal(r.status, expected, text); return JSON.parse(text);
  };
  const initialCatalog = await api("/api/model-services");
  assert.deepEqual(initialCatalog.services, []);
  assert.deepEqual(initialCatalog.connectionAgents.sort(), Object.keys(files).sort());
  const sources = [];
  for (const [index, server] of servers.entries()) {
    const provider = { id: `custom-${randomUUID()}`, name: `Source ${index + 1}`, vendor: "custom", protocol: index ? "chat-completions" : "messages", endpoint: `http://127.0.0.1:${server.address().port}${index ? "/v1" : ""}`, model: index ? "b" : "a", models: [index ? "b" : "a"], configured: true, credentialRef: null, credentialLabel: null, revision: 0, category: "local" };
    await api("/api/providers", provider, 200, "PUT"); sources.push({ ...provider, revision: 1 });
    assert.equal((await api(`/api/providers/${provider.id}/verify`, { expectedRevision: 1, model: provider.model })).state, "verified");
  }
  const models = sources.map((provider, index) => ({ id: randomUUID(), name: index ? "B" : "A", providerId: provider.id, model: provider.model, capabilities: { contextWindow: index ? 96000 : 48000, maxOutputTokens: index ? 8192 : 4096, tools: true, thinking: false, inputModalities: ["text"] } }));
  const draft = { id: randomUUID(), revision: 0, name: "Coding", defaultModelId: models[0].id, models };
  let service = await api("/api/model-services", draft, 200, "PUT");
  assert.equal(service.models.length, 2); assert.notEqual(service.models[0].provider.id, service.models[1].provider.id);
  await api("/api/model-services", draft, 409, "PUT");
  const unused = await api("/api/model-services", { ...draft, id: randomUUID(), name: "Unused" }, 200, "PUT");
  const transactions = {};
  for (const agentId of Object.keys(files)) {
    const config = (await api("/api/provider-status")).configs.find(c => c.agentId === agentId);
    const t = await api("/api/model-service-connections", { agentId, expectedRevision: config.revision, serviceId: service.id, expectedServiceRevision: service.revision });
    const attachmentsBeforeRetry = (await api("/api/model-services")).attachments.length;
    const retry = await api("/api/model-service-connections", { agentId, expectedRevision: config.revision, serviceId: service.id, expectedServiceRevision: service.revision });
    assert.equal(retry.id, t.id, "Same pending service preview must be reused");
    assert.equal((await api("/api/model-services")).attachments.length, attachmentsBeforeRetry, "Retry must not allocate another route or attachment");
    await api("/api/model-service-connections", { agentId, expectedRevision: config.revision, serviceId: unused.id, expectedServiceRevision: unused.revision }, 409);
    assert.equal((await api("/api/model-services")).attachments.length, attachmentsBeforeRetry, "A conflicting pending preview must not leave an orphan attachment");
    assert.equal(readFileSync(files[agentId], "utf8"), originals[agentId]);
    assert.equal(readFileSync(t.backupPath).subarray(0, 4).toString(), "LAC2");
    await api(`/api/provider-switches/${t.id}/apply`, {}); transactions[agentId] = t;
  }
  await api(`/api/model-services/${unused.id}/delete`, { expectedRevision: 1 });
  const cli = JSON.parse(readFileSync(files.claude));
  assert.equal(cli.modelPicker.options.length, 3); assert.equal(cli.env.CLAUDE_CODE_MAX_CONTEXT_TOKENS, "48000"); assert.equal(cli.env.CLAUDE_CODE_MAX_OUTPUT_TOKENS, "4096"); assert(cli.permissions.allow.includes("Read")); assert(cli.enabledPlugins.keep); assert.equal(cli.env.KEEP_ME, "yes");
  const opencode = JSON.parse(readFileSync(files.opencode)); const group = opencode.model.split("/")[0]; const local = opencode.provider[group].options; const route = local.baseURL + "/messages";
  assert(opencode.keep && opencode.provider.untouched && opencode.mcp.keep); assert.equal(Object.keys(opencode.provider[group].models).length, 3); assert.equal(opencode.provider[group].models[`local-agent-model-${models[1].id}`].limit.context, 96000);
  const installedHashes = Object.fromEntries(Object.entries(files).map(([a, p]) => [a, hash(p)]));
  const desktopConfig = JSON.parse(readFileSync(files["claude-desktop"]));
  assert.deepEqual(desktopConfig.coworkEgressAllowedHosts, ["example.invalid"]);
  assert.deepEqual(desktopConfig.inferenceModels.map(model => model.labelOverride), ["Coding · 默认", "A", "B"]);
  assert(desktopConfig.inferenceModels.every(model => /^claude-sonnet-5-local-agent-/.test(model.name)));
  const codexConfig = readFileSync(files.codex, "utf8");
  assert.match(codexConfig, /model = "local-agent-default"/);
  assert.match(codexConfig, /model_catalog_url = "http:\/\/127\.0\.0\.1:.*\/v1\/codex\/models\.json"/);
  assert.match(codexConfig, /\[mcp_servers\.keep\]/);
  const codexCatalogLine = codexConfig.match(/^model_catalog_json = (".*")$/m)[0];
  const codexCatalogFile = JSON.parse(codexCatalogLine.slice("model_catalog_json = ".length));
  assert.deepEqual(JSON.parse(readFileSync(codexCatalogFile)).models.map(m => m.slug), [
    "local-agent-default", ...models.map(m => `local-agent-model-${m.id}`),
  ]);
  // Older installs used only the HTTP catalog. Surface a directory sync for those installs.
  writeFileSync(files.codex, codexConfig.replace(codexCatalogLine + "\n", ""));
  assert.equal((await api("/api/model-services")).attachments.find(a => a.agentId === "codex").directorySyncRequired, true);
  writeFileSync(files.codex, codexConfig);
  assert.equal((await api("/api/model-services")).attachments.find(a => a.agentId === "codex").directorySyncRequired, false);
  for (const agentId of Object.keys(files)) {
    const provider = sources[1];
    const config = (await api("/api/provider-status")).configs.find(c => c.agentId === agentId);
    const direct = await api("/api/provider-switches", { agentId, expectedRevision: config.revision, providerId: provider.id, expectedProviderRevision: provider.revision, model: provider.model });
    assert.equal(hash(files[agentId]), installedHashes[agentId], "Direct selection only writes after confirmation");
    assert.equal(readFileSync(direct.backupPath).subarray(0, 4).toString(), "LAC2");
    await api(`/api/provider-switches/${direct.id}/apply`, {});
    assert.notEqual(hash(files[agentId]), installedHashes[agentId]);
    assert.equal((await api("/api/model-services")).attachments.some(a => a.agentId === agentId && a.attached), false);
    if (agentId === "claude") assert.equal(JSON.parse(readFileSync(files.claude)).modelPicker, undefined);
    if (agentId === "codex") assert.doesNotMatch(readFileSync(files.codex, "utf8"), /^model_catalog_json\s*=/m);
    const restore = await api(`/api/provider-switches/${direct.id}/restore`, {});
    await api(`/api/provider-switches/${restore.id}/apply`, {});
    assert.equal(hash(files[agentId]), installedHashes[agentId]);
  }
  const request = async (history, session, model = "local-agent-default", expected = 200, extra = {}) => {
    const r = await fetch(route, { method: "POST", headers: { Authorization: `Bearer ${local.apiKey}`, "Content-Type": "application/json", "x-opencode-session": session }, body: JSON.stringify({ model, max_tokens: 30, messages: history, ...extra }), signal: AbortSignal.timeout(20000) });
    const text = await r.text(); assert.equal(r.status, expected, text); return JSON.parse(text);
  };
  const changeBuiltin = async (disabledPluginIds, expected = 200) => {
    const view = await api("/api/builtin-plugins");
    return api("/api/builtin-plugins/changes", {operationId:randomUUID(),storeId:view.storeId,expectedRevision:view.revision,expectedGeneration:view.host.generation,expectedImpact:view.impact,composition:{...view.composition,disabledPluginIds}}, expected);
  };
  const callsBeforeStop = seen.length;
  await changeBuiltin(["builtin.model-services"]);
  await api("/api/model-services", undefined, 409);
  const stoppedCatalog = await fetch(local.baseURL + "/models", {headers:{"x-api-key":local.apiKey}});
  assert.equal(stoppedCatalog.status,503); await stoppedCatalog.text();
  const stoppedRequest = await fetch(route,{method:"POST",headers:{Authorization:`Bearer ${local.apiKey}`,"Content-Type":"application/json"},body:JSON.stringify({model:"local-agent-default",max_tokens:30,messages:[{role:"user",content:"must not send"}]})});
  assert.equal(stoppedRequest.status,503); await stoppedRequest.text();
  assert.equal(seen.length,callsBeforeStop,"A stopped model service must not send to an upstream");
  const blockedConnections=(await api("/api/snapshot")).agentConnections.items.filter(c=>c.endpoint===local.baseURL);
  assert(blockedConnections.length>0 && blockedConnections.every(c=>c.mode==="unknown" && c.reason.includes("插件")));
  assert(Object.entries(files).every(([agent,file])=>hash(file)===installedHashes[agent]),"Stopping a plugin must preserve Agent configuration files");
  await changeBuiltin([]);
  const catalog = await fetch(local.baseURL + "/models?limit=1000", { headers: { "x-api-key": local.apiKey } }); assert.equal(catalog.status, 200); assert.equal((await catalog.json()).data.length, 3);
  const page = await fetch(local.baseURL + "/models?limit=1", { headers: { "x-api-key": local.apiKey } }); const firstPage = await page.json(); assert(firstPage.has_more); assert.equal(firstPage.data.length, 1);
  const next = await fetch(local.baseURL + `/models?limit=1000&after_id=${firstPage.last_id}`, { headers: { "x-api-key": local.apiKey } }); assert.equal((await next.json()).data.length, 2);
  assert.equal((await fetch(local.baseURL + "/models?limit=0", { headers: { "x-api-key": local.apiKey } })).status, 401);
  assert.equal((await fetch(local.baseURL + "/models", { headers: { "x-api-key": "bad" } })).status, 401);
  assert.equal((await fetch(base + "/api/model-services")).status, 401);
  const installedClaude = readFileSync(files.claude);
  writeFileSync(files.claude, "{invalid");
  const unknown = (await api("/api/model-services")).attachments.find(a => a.agentId === "claude");
  assert.equal(unknown.configurationUnknown, true); assert.equal(unknown.attached, false);
  writeFileSync(files.claude, installedClaude);
  const installedOpenCode = readFileSync(files.opencode);
  writeFileSync(files.claude, originals.claude);
  writeFileSync(files.opencode, JSON.stringify({ ...opencode, model: "untouched/other" }));
  const retained = (await api("/api/model-services")).attachments.find(a => a.agentId === "opencode");
  assert.equal(retained.attached, false); assert.equal(retained.referenced, true);
  await api(`/api/model-services/${service.id}/delete`, { expectedRevision: service.revision }, 409);
  writeFileSync(files.claude, installedClaude); writeFileSync(files.opencode, installedOpenCode);
  await api(`/api/model-services/${service.id}/delete`, { expectedRevision: service.revision }, 409);
  const first = [{ role: "user", content: "one" }];
  hold = true; const arrived = new Promise(resolve => { arrive = resolve; }); const running = request(first, "ses_same"); await arrived;
  assert.equal((await changeBuiltin(["builtin.model-services"],503)).code,"busy","An in-flight response must prevent plugin shutdown");
  service = await api("/api/model-services", { ...draft, revision: 1, defaultModelId: models[1].id }, 200, "PUT"); release(); assert.equal((await running).model, "a");
  assert.equal(seen.at(-1).thinking, "disabled", "Native Messages service requests must explicitly disable upstream thinking defaults");
  const continuation = [...first, { role: "assistant", content: [{ type: "tool_use", id: "call-1", name: "read", input: {} }] }, { role: "user", content: [{ type: "tool_result", tool_use_id: "call-1", content: "fixture" }] }];
  assert.equal((await request(continuation, "ses_same", `local-agent-model-${models[1].id}`)).model, "a");
  const second = [...first, { role: "assistant", content: "OK" }, { role: "user", content: "two" }];
  assert.equal((await request(second, "ses_same")).model, "b");
  assert.equal((await request(first, "ses_specified", `local-agent-model-${models[0].id}`)).model, "a");
  assert.equal((await request(first, "ses_same")).model, "a");
  const before = seen.length;
  await request(first, "ses_unknown", "not-published", 400);
  await request(first, "ses_largeoutput", "local-agent-default", 400, { max_tokens: 9000 });
  await request(first, "ses_thinking", "local-agent-default", 400, { thinking: { type: "enabled", budget_tokens: 1000 } });
  await request([{ role: "user", content: "x".repeat(160000) }], "ses_contextdefault", "local-agent-default", 400);
  assert.equal(seen.length, before);
  assert.equal((await request([{ role: "user", content: "x".repeat(160000) }], "ses_contextspecified", `local-agent-model-${models[1].id}`)).model, "b");
  assert(Object.entries(files).every(([a, p]) => hash(p) === installedHashes[a]));
  await api("/api/dynamic-routes", { operationId: randomUUID(), bindingId: transactions.opencode.routing.id, expectedGeneration: "1", providerId: sources[0].id, expectedProviderRevision: 1, model: "a" }, 400);
  const extra = { ...models[0], id: randomUUID(), name: "Added" };
  service = await api("/api/model-services", { ...draft, revision: 2, defaultModelId: models[1].id, models: [...models, extra] }, 200, "PUT");
  await request(first, "ses_notsynced", `local-agent-model-${extra.id}`, 400);
  assert((await api("/api/model-services")).attachments.filter(a => a.attached).every(a => a.directorySyncRequired));
  const config = (await api("/api/provider-status")).configs.find(c => c.agentId === "opencode");
  const sync = await api("/api/model-service-connections", { agentId: "opencode", expectedRevision: config.revision, serviceId: service.id, expectedServiceRevision: service.revision });
  await api(`/api/provider-switches/${sync.id}/apply`, {});
  assert.notEqual(hash(files.opencode), installedHashes.opencode);
  const synced=JSON.parse(readFileSync(files.opencode)); assert.equal(synced.model.split("/")[0],group); assert.equal(synced.small_model,opencode.small_model); assert(synced.provider[synced.small_model.split("/")[0]]); assert.deepEqual(Object.keys(synced.provider).sort(),Object.keys(opencode.provider).sort());
  const undoSync = await api(`/api/provider-switches/${sync.id}/restore`, {}); await api(`/api/provider-switches/${undoSync.id}/apply`, {}); assert.equal(hash(files.opencode), installedHashes.opencode);
  const codexProviderId = () => readFileSync(files.codex, "utf8").match(/^model_provider = "([^"]+)"/m)[1];
  const installedCodexId = codexProviderId();
  const applyCodexService = async (target) => {
    const config = (await api("/api/provider-status")).configs.find(c => c.agentId === "codex");
    const preview = await api("/api/model-service-connections", { agentId: "codex", expectedRevision: config.revision, serviceId: target.id, expectedServiceRevision: target.revision });
    await api(`/api/provider-switches/${preview.id}/apply`, {});
    return preview;
  };
  const codexSync = await applyCodexService(service);
  assert.notEqual(codexSync.routing.id, transactions.codex.routing.id);
  assert.equal(codexProviderId(), installedCodexId, "Codex sessions keep the same provider ID across a catalog sync");
  assert(readFileSync(files.codex, "utf8").includes(`base_url = "${codexSync.routing.localEndpoint}"`));
  const alternate = await api("/api/model-services", { ...draft, id: randomUUID(), name: "Alternate" }, 200, "PUT");
  const alternateSwitch = await applyCodexService(alternate);
  assert.notEqual(codexProviderId(), installedCodexId, "Different model services retain separate provider identities");
  const returnSwitch = await applyCodexService(service);
  assert.equal(codexProviderId(), installedCodexId, "Returning to a model service restores its prior provider ID");
  for (const transaction of [returnSwitch, alternateSwitch, codexSync]) {
    const restore = await api(`/api/provider-switches/${transaction.id}/restore`, {});
    await api(`/api/provider-switches/${restore.id}/apply`, {});
  }
  assert.equal(hash(files.codex), installedHashes.codex, "Restore uses the original Codex config bytes");
  await api(`/api/model-services/${alternate.id}/delete`, { expectedRevision: alternate.revision });
  const requests = await api("/api/requests");
  const rows = Array.isArray(requests) ? requests : requests.items ?? requests.requests ?? [];
  assert(rows.some(r => r.routing?.service?.serviceId === service.id));
  for (const agent of Object.keys(files)) {
    const restore = await api(`/api/provider-switches/${transactions[agent].id}/restore`, {}); await api(`/api/provider-switches/${restore.id}/apply`, {}); assert.equal(readFileSync(files[agent], "utf8"), originals[agent]);
  }
  await api(`/api/model-services/${service.id}/delete`, { expectedRevision: service.revision });
  assert.equal((await api("/api/model-services")).services.length, 0);
  for (const [index, provider] of sources.entries()) {
    const chatModel = { ...models[index], capabilities: { ...models[index].capabilities, tools: false } };
    const chat = await api("/api/model-services", { id: randomUUID(), revision: 0, name: `Text ${index}`, defaultModelId: chatModel.id, models: [chatModel] }, 200, "PUT");
    const config = (await api("/api/provider-status")).configs.find(c => c.agentId === "opencode");
    const connect = await api("/api/model-service-connections", { agentId: "opencode", expectedRevision: config.revision, serviceId: chat.id, expectedServiceRevision: chat.revision });
    await api(`/api/provider-switches/${connect.id}/apply`, {});
    const installed = JSON.parse(readFileSync(files.opencode));
    const entry = installed.provider[installed.model.split("/")[0]];
    assert(Object.values(entry.models).every(model => model.tool_call === false), "Text-only metadata must not claim tool capability");
    const chatSession = `ses_chat${index}${randomUUID().replaceAll("-", "")}`;
    const chatRequest = async (extra, expected = 200, session = `ses_chat${index}${randomUUID().replaceAll("-", "")}`) => {
      const response = await fetch(entry.options.baseURL + "/messages", { method: "POST", headers: { Authorization: `Bearer ${entry.options.apiKey}`, "Content-Type": "application/json", "x-opencode-session": session }, body: JSON.stringify({ model: "local-agent-default", max_tokens: 30, messages: [{ role: "user", content: "text only" }], ...extra }), signal: AbortSignal.timeout(20000) });
      const body = await response.text(); assert.equal(response.status, expected, body); return JSON.parse(body);
    };
    const definitions = [{ name: "read", input_schema: { type: "object" } }];
    for (const tool_choice of [undefined, { type: "auto" }, { type: "none" }]) {
      await chatRequest({ tools: definitions, tool_choice }, 200, chatSession);
      assert.equal(seen.at(-1).protocol, provider.protocol);
      assert.equal(seen.at(-1).tools, undefined, "Optional tools must not reach a text-only upstream");
      assert.equal(seen.at(-1).toolChoice, undefined);
    }
    const beforeRejects = seen.length;
    for (const tool_choice of [{ type: "any" }, { type: "tool", name: "read" }, { type: "future" }]) {
      await chatRequest({ tools: definitions, tool_choice }, 400);
      await chatRequest({ tool_choice }, 400);
    }
    await chatRequest({ tools: [{ type: "web_search_20250305", name: "web_search", input_schema: {} }] }, 400);
    const history = { tools: definitions, messages: [{ role: "user", content: "text only" }, { role: "assistant", content: [{ type: "tool_use", id: "one", name: "read", input: {} }] }, { role: "user", content: [{ type: "tool_result", tool_use_id: "one", content: "keep" }] }] };
    await chatRequest(history, 400, chatSession);
    await chatRequest(history, 409);
    assert.equal(seen.length, beforeRejects, "Forced, unknown and historical tools must be rejected before sending");
    unexpectedTool = true;
    await chatRequest({ tools: definitions, tool_choice: { type: "auto" } }, 502);
    const streamed = await fetch(entry.options.baseURL + "/messages", { method: "POST", headers: { Authorization: `Bearer ${entry.options.apiKey}`, "Content-Type": "application/json", "x-opencode-session": `ses_stream${index}${randomUUID().replaceAll("-", "")}` }, body: JSON.stringify({ model: "local-agent-default", stream: true, max_tokens: 30, messages: [{ role: "user", content: "text only" }], tools: definitions }), signal: AbortSignal.timeout(20000) });
    const streamedBody = await streamed.text();
    assert([200, 502].includes(streamed.status), streamedBody);
    assert(streamedBody.includes("error"), streamedBody);
    assert(!streamedBody.includes('"type":"tool_use"'), "Unexpected upstream tool blocks must never reach the client");
    unexpectedTool = false;
    const restore = await api(`/api/provider-switches/${connect.id}/restore`, {});
    await api(`/api/provider-switches/${restore.id}/apply`, {});
    assert.equal(readFileSync(files.opencode, "utf8"), originals.opencode);
    await api(`/api/model-services/${chat.id}/delete`, { expectedRevision: chat.revision });
  }
  assert.equal((await api("/api/model-services")).services.length, 0);
  console.log(JSON.stringify({ passed: true, pluginStopAndRestore: true, pluginBusyResponseProtected: true, stoppedConnectionsUnknown: true, crossProviderCrud: true, staleEditorRejected: true, twoClientCatalogs: true, perModelCapabilities: true, authenticatedCatalogPagination: true, concurrentDefaultPublish: true, specifiedModelPreserved: true, toolTurnPinned: true, oldRequestReplay: true, unknownModelsAndInvalidCapabilitiesRejected: true, configHashesStableAcrossDefaultChange: true, directorySyncAndEncryptedByteRestore: true, serviceObservationRecorded: true, activeServiceDeletionRejected: true, textOnlyOptionalTools: true, forcedUnknownAndHistoricalToolsRejected: true, unexpectedUpstreamToolsBlocked: true, calls: seen.length }));
} finally {
  release?.();
  if (child && child.exitCode === null) { child.kill("SIGTERM"); await once(child, "exit"); }
  for (const server of servers) await new Promise(resolve => server.close(resolve));
  rmSync(directory, { recursive: true, force: true });
}
