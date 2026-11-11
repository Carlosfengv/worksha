import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { createInterface } from "node:readline";
import { once } from "node:events";
import http from "node:http";
import { DatabaseSync } from "node:sqlite";

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
let child, stderr = "", hold = false, arrive, release, unexpectedTool = false, limitedAccount;
let limitedAccountRetryAfter = "60", limitedAccountStatus = 429;
let accessRejection;
let replyFault;
let classifierReply = "1";
const classifierCalls = [];
const token = randomUUID();
try {
  for (const protocol of ["messages", "chat-completions"]) {
    const server = http.createServer(async (req, res) => {
      let raw = ""; for await (const chunk of req) raw += chunk;
      const body = JSON.parse(raw);
      const account = req.headers.authorization?.replace(/^Bearer /, "");
      const assistantNotice = body.messages?.some(message => message.role === "assistant" &&
        JSON.stringify(message.content).includes("【路由提示】"));
      seen.push({ protocol, model: body.model, thinking: body.thinking?.type, tools: body.tools, toolChoice: body.tool_choice, account, assistantNotice });
      if (JSON.stringify(body.system)?.includes("Classify the current request by its primary purpose")) {
        classifierCalls.push(body);
        const answer = classifierReply;
        if (answer === "timeout") await new Promise(resolve => setTimeout(resolve, 2400));
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ id: randomUUID(), type: "message", role: "assistant", model: body.model,
          content: [{ type: "text", text: answer }], stop_reason: "end_turn", usage: { input_tokens: 30, output_tokens: 1 } }));
        return;
      }
      if (limitedAccount && account === limitedAccount) {
        res.writeHead(limitedAccountStatus, { "Content-Type": "application/json", "Retry-After": limitedAccountRetryAfter });
        const type = limitedAccountStatus === 429 ? "rate_limit_error" : "api_error";
        res.end(JSON.stringify({ error: { type, message: "Fixture upstream rejection" } }));
        return;
      }
      if (accessRejection && accessRejection.account === account) {
        const { status, type, code, oversized, retryAfter } = accessRejection;
        const headers = { "Content-Type": "application/json" };
        if (retryAfter) headers["Retry-After"] = retryAfter;
        res.writeHead(status, headers);
        res.end(JSON.stringify({ error: { type, code, message: oversized ? "x".repeat(70_000) : "fixture-wire-secret" } }));
        return;
      }
      const fault = replyFault?.protocol === protocol && (!replyFault.account || replyFault.account === account) ? replyFault : undefined;
      if (fault?.error || (fault && !fault.format)) {
        if (fault.error) {
          const error = JSON.stringify({ error: fault.error });
          res.writeHead(200, { "Content-Type": fault.format === "json" ? "application/json" : "text/event-stream" });
          const prefix = fault.partialText === undefined ? "" : `data: ${JSON.stringify({ choices: [{ delta: { content: fault.partialText } }] })}\n\n`;
          res.end(fault.format === "json" ? error : `${prefix}data: ${error}\n\n`);
        } else {
          res.writeHead(200, { "Content-Type": body.stream ? "text/event-stream" : "application/json", "Content-Length": 4096 });
          res.write(body.stream ? 'data: {"error":{"type":"rate_limit_error"}}\n' : '{"id":"unfinished","choices":[');
          setTimeout(() => res.destroy(), 25);
        }
        return;
      }
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
      if (fault?.format ? fault.format === "sse" : body.stream) {
        const events = protocol === "messages"
          ? [
              { type: "message_start", message: { ...result, content: [], stop_reason: null, usage: { input_tokens: 5, output_tokens: 0 } } },
              { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
              { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "OK" } },
              { type: "content_block_stop", index: 0 },
              { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 2 } },
              { type: "message_stop" },
            ]
          : [
              { id: result.id, model: body.model, choices: [{ index: 0, delta: { role: "assistant", content: "OK" }, finish_reason: null }] },
              { id: result.id, model: body.model, choices: [{ index: 0, delta: {}, finish_reason: "stop" }], usage: result.usage },
            ];
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.end(events.map(event => `data: ${JSON.stringify(event)}\n\n`).join("") + (protocol === "messages" ? "" : "data: [DONE]\n\n"));
        return;
      }
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
  const detailFor = async (session, status) => {
    const query = new URLSearchParams({ agent: "opencode", session: `opencode:${session}` });
    for (let tries = 0; tries < 20; tries++) {
      const dashboard = await api(`/api/request-dashboard?${query}`);
      const record = dashboard.items.find(r => r.sessionId === `opencode:${session}`);
      if (record) {
        const detail = await api(`/api/request-detail?id=${encodeURIComponent(record.id)}`);
        if (detail.routeTrace && (status === undefined || detail.routeTrace.httpStatus === status)) {
          return { ...detail, summary: dashboard.summary };
        }
      }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.fail(`Missing route trace for ${session}`);
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
    if (agentId === "codex") {
      const fixedConfig = readFileSync(files.codex, "utf8");
      const fixedCatalogFile = JSON.parse(fixedConfig.match(/^model_catalog_json\s*=\s*(".*")$/m)[1]);
      assert.notEqual(fixedCatalogFile, codexCatalogFile);
      assert(JSON.parse(readFileSync(fixedCatalogFile)).models.some(model => model.slug === provider.model && model.priority === 0));
    }
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
  await api("/api/model-services/routing-projects", undefined, 409);
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
  // The first pool account uses the Provider's existing credential reference.
  // Pool enrollment still needs its own verification.
  const alternatePoolModel = "pool-model-alternate";
  const defaultPoolCredential = await api("/api/provider-credentials", {
    label: "Account A",
    secret: "fixture-pool-a",
  });
  const poolProvider = {
    ...sources[0],
    id: `custom-${randomUUID()}`,
    name: "Pool only",
    models: [...sources[0].models, alternatePoolModel],
    credentialRef: defaultPoolCredential.credentialRef,
    credentialLabel: "Account A",
  };
  await api("/api/providers", { ...poolProvider, revision: 0 }, 200, "PUT");
  const poolPath = `/api/provider-upstream-accounts/${poolProvider.id}`;
  const { storeId } = await api(poolPath, { kind: "list" });
  const pool = [];
  const poolCredentials = [];
  for (const [key, label] of [["fixture-pool-a", "Account A"], ["fixture-pool-b", "Account B"]]) {
    const { credentialRef } = pool.length === 0
      ? defaultPoolCredential
      : await api("/api/provider-credentials", { label, secret: key });
    const id = randomUUID();
    const input = { kind: "save", storeId, account: { id, expectedRevision: 0, providerRevision: poolProvider.revision, label, credentialRef } };
    const saved = await api(poolPath, input);
    assert.deepEqual(await api(poolPath, input), saved, "Lost save replies can be recovered with the same account ID");
    assert(!JSON.stringify(saved).includes(credentialRef), "Account catalog must omit credential references");
    const account = saved.accounts.find(a => a.id === id);
    assert.deepEqual(account.verifications, []);
    assert.deepEqual(account.cooldowns, []);
    pool.push({ id, revision: account.revision });
    poolCredentials.push(credentialRef);
  }
  const pooledDraft = {
    ...draft,
    id: randomUUID(),
    name: "Account pool",
    models: [{ ...models[0], providerId: poolProvider.id }, models[1]],
    routingPolicy: { enabled: true, accountPools: [{ modelId: models[0].id, accounts: pool }] },
  };
  const previewService = (draft, entry = { kind: "default" }, expected = 200, origin = storeId) =>
    api("/api/model-services/preview", { storeId: origin, draft, entry }, expected);
  const beforePreview = seen.length;
  const initialPreview = await previewService(pooledDraft);
  assert.equal(initialPreview.storeId, storeId);
  assert.deepEqual(initialPreview.models.map(model => model.scope), ["preferred", "outsideFallback"]);
  assert.deepEqual(initialPreview.models[0].accounts.map(a => [a.label, a.state, a.verification, a.verifyOnPublish]), [
    ["Account A", "configured", "unverified", true], ["Account B", "configured", "unverified", true],
  ]);
  assert(!JSON.stringify(initialPreview).includes(poolCredentials[0]));
  assert(!JSON.stringify(initialPreview).includes(pool[0].id));
  assert(!JSON.stringify(initialPreview).includes(poolProvider.endpoint));
  const enabledFallback = { ...pooledDraft, routingPolicy: { ...pooledDraft.routingPolicy, fallbackAcrossModels: true } };
  assert.deepEqual((await previewService(enabledFallback)).models.map(m => m.scope), ["preferred", "fallback"]);
  const specifiedPreview = await previewService(enabledFallback, { kind: "specified", modelId: models[1].id });
  assert.deepEqual(specifiedPreview.models.map(m => m.scope), ["preferred", "capabilityMismatch"]);
  const previewRule = { id: randomUUID(), name: "Preview rule", modelId: models[1].id, stream: true };
  const conditionalPreview = await previewService({
    ...pooledDraft, routingPolicy: { ...pooledDraft.routingPolicy, rules: [previewRule] },
  }, { kind: "rule", ruleId: previewRule.id });
  assert.deepEqual(conditionalPreview.models.map(m => m.scope), ["preferred", "outsideFallback"]);
  await previewService(pooledDraft, { kind: "default" }, 409, randomUUID());
  const unauthorizedPreview = await fetch(base + "/api/model-services/preview", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ storeId, draft: pooledDraft, entry: { kind: "default" } }),
    signal: AbortSignal.timeout(20_000),
  });
  assert.equal(unauthorizedPreview.status, 401);
  assert.equal(seen.length, beforePreview, "Preview never sends an upstream probe");
  assert.deepEqual((await api("/api/model-services")).services, [], "Preview does not publish the draft");
  await api("/api/model-services", pooledDraft, 400, "PUT");
  const verifyA = { kind: "verify", storeId, account: pool[0], expectedRevision: poolProvider.revision, model: poolProvider.model };
  const beforeAccountVerify = seen.length;
  await api(poolPath, { ...verifyA, storeId: randomUUID() }, 409);
  await api(poolPath, { ...verifyA, expectedRevision: 0 }, 409);
  assert.equal(seen.length, beforeAccountVerify, "Store and provider version are checked before a probe");
  const verifiedA = await api(poolPath, verifyA);
  const firstCheck = verifiedA.accounts.find(a => a.id === pool[0].id).verifications[0];
  assert.equal(firstCheck.model, poolProvider.model);
  assert.equal(firstCheck.state, "verified");
  assert(Number.isFinite(Date.parse(firstCheck.checkedAt)));
  assert.deepEqual(verifiedA.accounts.find(a => a.id === pool[1].id).verifications, []);
  assert(!JSON.stringify(verifiedA).includes("fingerprint"), "Account verification details omit internal hashes");
  assert(!JSON.stringify(verifiedA).includes("fixture-pool-a"), "Account verification details omit keys");
  assert.equal(seen.at(-1).account, "fixture-pool-a");
  const rotatedKey = await api("/api/provider-credentials", { label: "Rotation", secret: "fixture-pool-rotation" });
  for (const credentialRef of [rotatedKey.credentialRef, poolCredentials[0]]) {
    const updated = await api(poolPath, { kind: "save", storeId, account: {
      id: pool[0].id, expectedRevision: pool[0].revision, providerRevision: poolProvider.revision, label: "Account A", credentialRef,
    } });
    const changed = updated.accounts.find(a => a.id === pool[0].id);
    assert.deepEqual(changed.verifications, [{ ...firstCheck, state: "outdated" }]);
    assert.equal((await previewService(pooledDraft)).models[0].accounts[0].state, "changed");
    pool[0] = { id: changed.id, revision: changed.revision };
  }
  const beforeStaleAccount = seen.length;
  await api(poolPath, verifyA, 400);
  assert.equal(seen.length, beforeStaleAccount, "Stale account versions cannot send a probe");
  limitedAccount = "fixture-pool-a";
  const failed = await api(poolPath, { ...verifyA, account: pool[0], model: alternatePoolModel });
  const failedA = failed.accounts.find(a => a.id === pool[0].id);
  assert.equal(failedA.verifications.find(v => v.model === alternatePoolModel).state, "failed");
  assert.deepEqual(failedA.cooldowns, [], "A manual probe does not write routing cooldowns");
  assert.equal(seen.at(-1).model, alternatePoolModel);
  limitedAccount = undefined;
  const verifiedB = await api("/api/model-service-members/verify", {
    storeId: initialCatalog.writeProtocol.storeId, providerId: poolProvider.id, expectedRevision: poolProvider.revision, model: poolProvider.model, upstreamAccount: pool[1],
  });
  assert.equal(verifiedB.state, "verified");
  assert.equal(seen.at(-1).account, "fixture-pool-b");
  await api("/api/model-services", pooledDraft, 400, "PUT");
  await api(poolPath, { ...verifyA, account: pool[0] });
  let pooledService = await api("/api/model-services", pooledDraft, 200, "PUT");
  const publishedPreview = await previewService({ ...pooledDraft, revision: pooledService.revision });
  assert(publishedPreview.models[0].accounts.every(a => a.verification === "verified" && !a.verifyOnPublish));
  await previewService(pooledDraft, { kind: "default" }, 409);
  const poolConfig = (await api("/api/provider-status")).configs.find(c => c.agentId === "opencode");
  const poolConnect = await api("/api/model-service-connections", {
    agentId: "opencode", expectedRevision: poolConfig.revision, serviceId: pooledService.id, expectedServiceRevision: pooledService.revision,
  });
  await api(`/api/provider-switches/${poolConnect.id}/apply`, {});
  const poolInstalled = JSON.parse(readFileSync(files.opencode));
  const poolEndpoint = poolInstalled.provider[poolInstalled.model.split("/")[0]].options;
  const callPool = async (messages, session, expected = 200, extra = {}) => {
    const headers = { Authorization: `Bearer ${poolEndpoint.apiKey}`, "Content-Type": "application/json" };
    if (session) headers["x-opencode-session"] = session;
    const response = await fetch(poolEndpoint.baseURL + "/messages", {
      method: "POST", headers,
      body: JSON.stringify({ model: "local-agent-default", max_tokens: 30, messages, ...extra }),
      signal: AbortSignal.timeout(20000),
    });
    const body = await response.text();
    assert.equal(response.status, expected, body);
    if (expected === 200 && extra.stream) return body;
    return JSON.parse(body);
  };
  const installedPoolHash = hash(files.opencode);
  const publishPool = async (settings = {}, policy = {}) => {
    pooledService = await api("/api/model-services", {
      ...pooledDraft, revision: pooledService.revision,
      routingPolicy: {
        ...pooledDraft.routingPolicy, ...policy,
        accountPools: [{ ...pooledDraft.routingPolicy.accountPools[0], ...settings }],
      },
    }, 200, "PUT");
    assert.equal(hash(files.opencode), installedPoolHash, "Account routing publishes without editing client config");
  };
  const assertPool = async (messages, session, account) => {
    const before = seen.length;
    const reply = await callPool(messages, session);
    assert.equal(reply.model, poolProvider.model);
    assert.deepEqual(seen.slice(before).map(r => r.account), [`fixture-pool-${account}`]);
    return reply;
  };
  const accessAccounts = [];
  for (const suffix of ["a", "b", "c", "rate"]) {
    const label = `Access ${suffix}`;
    const { credentialRef } = await api("/api/provider-credentials", { label, secret: `fixture-access-${suffix}` });
    const id = randomUUID();
    const saved = await api(poolPath, { kind: "save", storeId, account: {
      id, expectedRevision: 0, providerRevision: poolProvider.revision, label, credentialRef,
    } });
    const account = { id, revision: saved.accounts.find(a => a.id === id).revision };
    accessAccounts.push(account);
    await api(poolPath, { ...verifyA, account });
  }
  const rateAccount = accessAccounts.pop();
  const accessPolicy = { fallbackOnAccessDenied: true, cooldownSeconds: 180 };
  const replyPolicy = { ...accessPolicy, fallbackAcrossModels: true, cooldownSeconds: 1 };
  await publishPool({ accounts: accessAccounts }, replyPolicy);
  for (const [index, { stream, format }] of [{ stream: true, format: "json" }, { stream: false, format: "sse" }].entries()) {
    const session = `ses_mismatchedreply${index}`;
    const before = seen.length;
    replyFault = { protocol: "messages", account: "fixture-access-a", format };
    const result = await callPool(first, session, 502, { stream });
    assert.match(result.error.message, /流式格式.*上游结果未确认/);
    const detail = await detailFor(session);
    assert.equal(detail.request.attempts.length, 1);
    assert.equal(detail.request.attempts[0].httpStatus, 200);
    assert.equal(detail.request.attempts[0].status, "interrupted");
    assert.equal(detail.routeTrace.httpStatus, 502);
    assert(detail.routeTrace.decisions.some(d => d.reason === "upstream-outcome-unconfirmed"));
    if (format === "json") assert.equal(detail.request.attempts[0].usage.outputTokens, 2);
    replyFault = undefined;
    await callPool(first, session, 409, { stream: !stream });
    assert.deepEqual(seen.slice(before).map(r => r.account), ["fixture-access-a"]);
  }
  console.log(JSON.stringify({ mismatchedReplyStopsWithoutFallback: true, preservesKnownUsage: true, replayBlocked: true }));
  for (const [index, { protocol, stream }] of [
    { protocol: "messages", stream: false },
    { protocol: "chat-completions", stream: false },
    { protocol: "chat-completions", stream: true },
  ].entries()) {
    const converted = protocol === "chat-completions";
    if (index === 1) {
      await publishPool({ accounts: accessAccounts }, {
        ...replyPolicy,
        rules: [{ id: randomUUID(), name: "Use converted reply", modelId: models[1].id, agent: "opencode" }],
      });
    }
    const account = converted ? undefined : "fixture-access-a";
    replyFault = { protocol, account };
    const session = `ses_replyunknown${index}`;
    const before = seen.length;
    const result = await callPool(first, session, 502, { stream });
    assert.match(result.error.message, /上游结果未确认/);
    const detail = await detailFor(session);
    assert.equal(detail.request.attempts.length, 1);
    assert(detail.routeTrace.decisions.some(d => d.reason === "upstream-outcome-unconfirmed"));
    assert(detail.routeTrace.decisions.some(d => d.reason === "outcome-unknown"), session);
    await callPool(first, session, 409, { stream });
    if (!converted) {
      replyFault = undefined;
      const compacted = [{ role: "user", content: "Compacted account-pool history" }];
      const blocked = await callPool(compacted, session, 409);
      assert.match(blocked.error.message, /原请求.*尚未确认/);
    }
    assert.deepEqual(seen.slice(before).map(r => [r.protocol, r.account]), [[protocol, account]]);
  }
  const beforeConvertedDenial = seen.length;
  replyFault = { protocol: "chat-completions", error: { type: "authentication_error", code: "invalid_api_key" } };
  await callPool(first, "ses_converteddenial", 401, { stream: true });
  assert.deepEqual(seen.slice(beforeConvertedDenial).map(r => r.protocol), ["chat-completions"]);
  const beforeConvertedPartial = seen.length;
  replyFault = { protocol: "chat-completions", partialText: "PARTIAL_CONVERTED", error: { type: "rate_limit_error" } };
  const convertedPartial = await callPool(first, "ses_convertedpartial", 200, { stream: true });
  assert.match(convertedPartial, /PARTIAL_CONVERTED/);
  assert.match(convertedPartial, /"type":"error"/);
  assert.doesNotMatch(convertedPartial, /"type":"message_stop"/);
  assert.deepEqual(seen.slice(beforeConvertedPartial).map(r => r.protocol), ["chat-completions"]);
  await new Promise(resolve => setTimeout(resolve, 1100));
  const beforeConvertedRateLimit = seen.length;
  replyFault = { protocol: "chat-completions", error: { type: "rate_limit_error" } };
  const convertedFallback = await callPool(first, "ses_convertedratelimit", 200, { stream: true });
  assert.match(convertedFallback, /"text":"OK"/);
  assert.deepEqual(seen.slice(beforeConvertedRateLimit).map(r => r.protocol), ["chat-completions", "messages"]);
  replyFault = undefined;
  // A canonical SSE rate-limit rejection carries the default 30-second Retry-After.
  const convertedRecoveryReady = Date.now() + 31_000;
  console.log(JSON.stringify({ interruptedAccountReplyNotRetried: true, compactedAccountHistoryBlocked: true, interruptedConvertedRepliesNotRetried: true, convertedDenialNotRetried: true, convertedRateLimitStillFallsBack: true }));
  await publishPool({ accounts: accessAccounts }, accessPolicy);
  const singleAttemptErrors = [
    { status: 401, type: "authentication_error" },
    { status: 403, type: "permission_error", oversized: true },
    { status: 400, type: "invalid_request_error", oversized: true },
    { status: 401, type: "permission_error", code: "invalid_api_key" },
    { status: 403, type: "authentication_error", code: "model_access_denied" },
    { status: 502, type: "content_policy_violation", code: "provider_error" },
  ];
  for (const [index, error] of singleAttemptErrors.entries()) {
    accessRejection = { account: "fixture-access-a", ...error };
    const before = seen.length;
    const result = await callPool(first, `ses_accessunknown${index}`, error.status);
    assert.deepEqual(seen.slice(before).map(r => r.account), ["fixture-access-a"]);
    if ([401, 403].includes(error.status)) {
      assert(!JSON.stringify(result).includes("fixture-wire-secret"));
    }
  }
  const listAccessAccounts = async () => (await api(poolPath, { kind: "list" })).accounts;
  const cooldownsFor = async (account) => (await listAccessAccounts()).find(a => a.id === account.id).cooldowns;
  assert.deepEqual(await cooldownsFor(accessAccounts[0]), [], "Ambiguous denials do not poison other routes");
  await publishPool({ accounts: [rateAccount, accessAccounts[1]] }, {
    ...accessPolicy,
    fallbackOnRateLimit: false,
  });
  accessRejection = {
    account: "fixture-access-rate",
    status: 429,
    type: "rate_limit_error",
    oversized: true,
    retryAfter: "27",
  };
  const beforeOversizedRate = seen.length;
  const beforeOversizedRateAt = Date.now();
  await callPool(first, "ses_accessoversizedrate", 429);
  assert.deepEqual(seen.slice(beforeOversizedRate).map(r => r.account), ["fixture-access-rate"]);
  const [oversizedRateCooldown] = await cooldownsFor(rateAccount);
  assert.equal(oversizedRateCooldown.reason, "upstream_rate_limit");
  assert(oversizedRateCooldown.untilMs >= beforeOversizedRateAt + 27_000);
  assert(oversizedRateCooldown.untilMs <= Date.now() + 27_000);
  console.log(JSON.stringify({ unreadableErrorPreservesStatus: true, retryAfterPreserved: true, contradictoryDenialStops: true }));

  await publishPool({ accounts: accessAccounts }, { cooldownSeconds: 180 });
  accessRejection = { account: "fixture-access-a", status: 403, code: "model_access_denied" };
  const beforeDenied = seen.length;
  await callPool(first, "ses_accessunapproved", 403);
  assert.deepEqual(seen.slice(beforeDenied).map(r => r.account), ["fixture-access-a"], "Earlier publications require explicit opt-in");
  const [modelDenial] = await cooldownsFor(accessAccounts[0]);
  assert.equal(modelDenial.model, poolProvider.model);
  assert.equal(modelDenial.reason, "upstream_model_access_denied");
  const beforeCachedDenial = seen.length;
  await callPool(first, "ses_accesscachedunapproved", 403);
  assert.equal(seen.length, beforeCachedDenial, "A cached rejection cannot authorize fallback");
  await publishPool({ accounts: accessAccounts }, accessPolicy);
  const beforeApproved = seen.length;
  await callPool(first, "ses_accessapproved");
  assert.deepEqual(seen.slice(beforeApproved).map(r => r.account), ["fixture-access-b"], "A model-scoped denial skips the account for that model");
  const mixedTrace = await detailFor("ses_accessapproved");
  assert.equal(mixedTrace.request.attempts.length, 1);
  assert.equal(mixedTrace.summary.attempts, 1);
  assert.equal(mixedTrace.routeTrace.httpStatus, 200);
  assert(mixedTrace.routeTrace.decisions.some(d => d.reason === "access-model-denied" && d.accountPosition === 1));
  assert(mixedTrace.routeTrace.decisions.some(d => d.reason === "earlier-candidate-accepted" && d.accountPosition === 3));
  assert(!JSON.stringify(mixedTrace.routeTrace).includes("fixture-access-a"));
  assert(!JSON.stringify(mixedTrace.routeTrace).includes(accessAccounts[0].id));

  await publishPool({ accounts: [accessAccounts[2], accessAccounts[1]] }, accessPolicy);
  accessRejection = { account: "fixture-access-c", status: 401, code: "invalid_api_key" };
  const beforeInvalid = seen.length;
  await callPool(first, "ses_accesscredential");
  assert.deepEqual(seen.slice(beforeInvalid).map(r => r.account), ["fixture-access-c", "fixture-access-b"]);
  const [credentialDenial] = await cooldownsFor(accessAccounts[2]);
  assert.equal(credentialDenial.model, null);
  assert.equal(credentialDenial.reason, "upstream_credential_rejected");
  await publishPool({ accounts: [accessAccounts[2]] }, accessPolicy);
  const accessHealth = (await api("/api/model-services")).routingHealth.find(
    row => row.serviceId === pooledService.id && row.modelId === models[0].id,
  ).status;
  assert.equal(accessHealth.state, "cooling");
  assert.equal(accessHealth.reason, "upstream_credential_rejected");
  const beforeNoCross = seen.length;
  await callPool(first, "ses_accessnocross", 401);
  assert.equal(seen.length, beforeNoCross, "Account fallback does not grant cross-model fallback");
  const zeroTrace = await detailFor("ses_accessnocross");
  assert.equal(zeroTrace.request.status, "failed");
  assert.deepEqual(zeroTrace.request.attempts, []);
  assert.equal(zeroTrace.summary.attempts, 0);
  assert.equal(zeroTrace.request.usage.totalTokens, null);
  assert.equal(zeroTrace.routeTrace.httpStatus, 401);
  assert(zeroTrace.routeTrace.decisions.some(d => d.reason === "cross-model-fallback-disabled"));
  assert(zeroTrace.routeTrace.decisions.some(d => d.reason === "access-credential-rejected"));
  const unauthorizedTrace = await fetch(base + `/api/request-detail?id=${encodeURIComponent(zeroTrace.request.id)}`, {
    headers: { Authorization: `Bearer ${poolEndpoint.apiKey}` },
    signal: AbortSignal.timeout(20_000),
  });
  assert.equal(unauthorizedTrace.status, 401, "A model gateway token cannot read request decisions");
  await publishPool({ accounts: [accessAccounts[2]] }, { ...accessPolicy, fallbackAcrossModels: true });
  await new Promise(resolve => setTimeout(resolve, Math.max(0, convertedRecoveryReady - Date.now())));
  assert.equal((await callPool(first, "ses_accesscross")).model, sources[1].model);
  assert.deepEqual(seen.slice(beforeNoCross).map(r => r.model), [sources[1].model]);

  await publishPool({ accounts: [accessAccounts[1], ...accessAccounts.filter(a => a !== accessAccounts[1])], affinity: "strict" }, { ...accessPolicy, fallbackAcrossModels: true });
  await callPool(first, "ses_accessstrict");
  accessRejection = { account: "fixture-access-b", status: 401, code: "invalid_api_key" };
  const beforePinnedDenial = seen.length;
  await callPool(continuation, "ses_accessstrict", 401);
  await callPool(second, "ses_accessstrict", 401);
  assert.deepEqual(seen.slice(beforePinnedDenial).map(r => r.account), ["fixture-access-b"], "A pinned continuation reports rejection and a strict session never switches accounts or models");
  accessRejection = undefined;
  const accessRoutes = await api("/api/requests");
  const accessRows = Array.isArray(accessRoutes) ? accessRoutes : accessRoutes.items ?? accessRoutes.requests ?? [];
  assert(accessRows.some(r => r.routing?.service?.fallbackReason === "upstream_credential_rejected"));
  assert(accessRows.some(r => r.routing?.service?.fallbackReason === "upstream_model_access_denied"));
  console.log(JSON.stringify({ accessFallback: true, requiresPublishedOptIn: true, ambiguousAndOversizedDenialsStop: true, scopedDenialCooldowns: true, strictAndToolContinuationPinned: true }));

  await publishPool({ strategy: "round-robin" });
  await assertPool(first, "ses_roundfirst", "a");
  await assertPool(first, "ses_roundfirst", "a");
  await assertPool(first, "ses_roundsecond", "b");
  const rotated = await assertPool(second, "ses_roundsecond", "a");
  assert(!rotated.content.some(part => part.type === "text" && part.text.includes("已恢复使用首选账号")));
  await assertPool(first, "ses_roundthird", "b");
  await publishPool({ strategy: "weighted", weights: { [pool[0].id]: 3 } });
  for (const [index, account] of ["a", "a", "b", "a"].entries()) {
    await assertPool(first, `ses_weight${index}`, account);
  }
  for (const [index, status, settings, resumedOrder] of [
    [0, 429, { strategy: "round-robin" }, ["a", "b"]],
    [1, 503, { strategy: "weighted", weights: { [pool[0].id]: 3 } }, ["a", "a", "b", "a"]],
    [2, 429, { strategy: "quota-remaining" }, ["a"]],
  ]) {
    await publishPool();
    limitedAccount = "fixture-pool-a";
    limitedAccountRetryAfter = "1";
    limitedAccountStatus = status;
    const recoverySession = `ses_poolrecoverytrigger${index}`;
    const fallback = await callPool(first, recoverySession);
    const notice = fallback.content.filter(part => part.type === "text" && part.text.startsWith("【路由提示】"));
    const reason = status === 429 ? "首选来源限流" : "首选来源暂不可用";
    assert.deepEqual(notice.map(part => part.text.trim()), [
      `【路由提示】${reason}，本轮已切换账号，继续使用 Pool only / ${poolProvider.model}。`,
    ]);
    const row = (await api(poolPath, { kind: "list" })).accounts.find(a => a.id === pool[0].id);
    const [cooldown] = row.cooldowns;
    assert.equal(cooldown.reason, status === 429 ? "upstream_rate_limit" : "upstream_failure");
    assert.equal(cooldown.model, status === 429 ? null : poolProvider.model);
    limitedAccount = undefined;
    await new Promise(resolve => setTimeout(resolve, Math.max(0, cooldown.untilMs - Date.now()) + 50));
    hold = true;
    const arrived = new Promise(resolve => { arrive = resolve; });
    const probe = callPool([...first, { role: "assistant", content: fallback.content }, second.at(-1)], recoverySession);
    await Promise.race([arrived, probe.then(() => assert.fail("Recovery probe was not held"))]);
    try {
      assert.equal(seen.at(-1).account, "fixture-pool-a");
      await publishPool(settings);
      for (let request = 0; request < 3; request++) {
        const session = `ses_poolrecoveryconcurrent${index}x${request}`;
        await assertPool(first, session, "b");
        const detail = await detailFor(session, 200);
        const [attempt] = detail.request.attempts;
        assert.equal(detail.request.attempts.length, 1);
        assert.equal(attempt.routing.service.upstreamAccountLabel, "Account B");
        assert.equal(attempt.routing.service.fallbackReason ?? null, null,
          "A probing account must be excluded before allocation, including across publications");
        assert(detail.routeTrace.decisions.some(decision =>
          decision.accountPosition === 1 && decision.reason === "recovery-in-progress"));
      }
    } finally {
      release();
      const restored = await probe;
      assert.equal(restored.model, poolProvider.model);
      const notice = restored.content.filter(part => part.type === "text" && part.text.startsWith("【路由提示】"));
      assert.deepEqual(notice.map(part => part.text.trim()), [
        `【路由提示】已恢复使用首选账号，继续使用 Pool only / ${poolProvider.model}。`,
      ]);
    }
    for (const [request, account] of resumedOrder.entries()) {
      await assertPool(first, `ses_poolrecovered${index}x${request}`, account);
    }
  }
  limitedAccountRetryAfter = "60";
  limitedAccountStatus = 429;
  console.log(JSON.stringify({ probingAccountsExcludedBeforeAllocation: true, recoveredAccountsRejoinSchedule: true }));
  await publishPool({ strategy: "quota-remaining", accounts: [...pool].reverse() });
  await assertPool(first, "ses_unknownquota", "b");
  await publishPool({ strategy: "round-robin", affinity: "prefer" });
  await assertPool(first, "ses_preferone", "a");
  await assertPool(first, "ses_prefertwo", "b");
  await assertPool(second, "ses_prefertwo", "b");
  await assertPool(first, "ses_preferthree", "a");
  await publishPool({ strategy: "round-robin", affinity: "strict" }, { fallbackAcrossModels: true });
  await assertPool(first, "ses_strictone", "a");
  await api(poolPath, { kind: "set-state", storeId, account: pool[0], expectedState: "enabled", state: "paused" });
  assert.equal((await previewService({ ...pooledDraft, revision: pooledService.revision })).models[0].accounts[0].state, "paused");
  const beforeStrict = seen.length;
  await api(poolPath, { ...verifyA, account: pool[0] }, 400);
  assert.equal(seen.length, beforeStrict, "Paused account cannot send a manual probe");
  await callPool(second, "ses_strictone", 503);
  assert.equal(seen.length, beforeStrict, "Strict affinity cannot switch accounts or models when pinned account is paused");
  await assertPool(first, "ses_stricttwo", "b");
  await api(poolPath, { kind: "set-state", storeId, account: pool[0], expectedState: "paused", state: "enabled" });
  await publishPool({ affinity: "strict", accounts: pool.slice(1) }, { fallbackAcrossModels: true });
  const beforeRemoved = seen.length;
  await callPool([...second, { role: "assistant", content: "OK" }, { role: "user", content: "three" }], "ses_strictone", 409);
  assert.equal(seen.length, beforeRemoved, "Publishing a pool without the pinned revision cannot silently reassign a strict session");
  await publishPool({ affinity: "strict" });
  await assertPool(first, "ses_strictModelPublication", "a");
  await assertPool(first, "ses_strictSourcePublication", "a");
  for (const account of pool) {
    assert.equal((await api(poolPath, { ...verifyA, account, model: alternatePoolModel }))
      .accounts.find(item => item.id === account.id)
      .verifications.find(verification => verification.model === alternatePoolModel).state, "verified");
  }
  pooledService = await api("/api/model-services", {
    ...pooledDraft, revision: pooledService.revision,
    models: [{ ...pooledDraft.models[0], model: alternatePoolModel }, pooledDraft.models[1]],
    routingPolicy: {
      ...pooledDraft.routingPolicy,
      accountPools: [{ modelId: models[0].id, accounts: [...pool].reverse(), affinity: "strict" }],
    },
  }, 200, "PUT");
  const beforePublishedModel = seen.length;
  assert.equal((await callPool(second, "ses_strictModelPublication")).model, alternatePoolModel);
  assert.deepEqual(seen.slice(beforePublishedModel).map(r => r.account), ["fixture-pool-a"],
    "Publishing a new upstream model preserves the still-authorized strict account");
  assert.equal((await callPool(first, "ses_strictModelFresh")).model, alternatePoolModel);
  assert.equal(seen.at(-1).account, "fixture-pool-b", "New sessions follow the new account order");

  const replacementPath = `/api/provider-upstream-accounts/${sources[0].id}`;
  const replacementCredential = await api("/api/provider-credentials", {
    label: "Replacement source account", secret: "fixture-strict-source",
  });
  const replacementId = randomUUID();
  const replacementCatalog = await api(replacementPath, {
    kind: "save", storeId,
    account: {
      id: replacementId, expectedRevision: 0, providerRevision: sources[0].revision,
      label: "Replacement source account", credentialRef: replacementCredential.credentialRef,
    },
  });
  const replacementAccount = {
    id: replacementId, revision: replacementCatalog.accounts.find(account => account.id === replacementId).revision,
  };
  await api(replacementPath, {
    kind: "verify", storeId, account: replacementAccount, expectedRevision: sources[0].revision, model: sources[0].model,
  });
  pooledService = await api("/api/model-services", {
    ...pooledDraft, revision: pooledService.revision,
    models: [{ ...pooledDraft.models[0], providerId: sources[0].id }, pooledDraft.models[1]],
    routingPolicy: {
      ...pooledDraft.routingPolicy, fallbackAcrossModels: true,
      accountPools: [{ modelId: models[0].id, accounts: [replacementAccount], affinity: "strict" }],
    },
  }, 200, "PUT");
  const beforePublishedSource = seen.length;
  await callPool(second, "ses_strictSourcePublication", 409);
  assert.equal(seen.length, beforePublishedSource, "A replacement Provider cannot reassign an existing strict session");
  const sourceTrace = await detailFor("ses_strictSourcePublication", 409);
  assert.equal(sourceTrace.routeTrace.sentUpstream, false);
  assert(sourceTrace.routeTrace.decisions.some(decision => decision.reason === "strict-affinity"));
  assert.equal((await callPool(first, "ses_strictSourceFresh")).model, sources[0].model);
  assert.equal(seen.at(-1).account, "fixture-strict-source");
  assert.equal(hash(files.opencode), installedPoolHash);
  console.log(JSON.stringify({ strictAccountSurvivesModelPublication: true, replacementSourceDoesNotReassign: true }));
  await publishPool();
  limitedAccount = "fixture-pool-a";
  limitedAccountRetryAfter = "20";
  const beforePool = seen.length;
  await callPool(first, null, 409);
  assert.equal(seen.length, beforePool, "An unknown session cannot bypass the published pool");
  const poolFallback = await callPool(first, "ses_poolsame");
  assert.equal(poolFallback.model, poolProvider.model);
  assert.deepEqual(seen.slice(beforePool).map(r => r.account), ["fixture-pool-a", "fixture-pool-b"]);
  const cooled = (await api(poolPath, { kind: "list" })).accounts;
  const [accountCooldown] = cooled.find(a => a.id === pool[0].id).cooldowns;
  assert.equal(accountCooldown.model, null);
  assert.equal(accountCooldown.reason, "upstream_rate_limit");
  assert(accountCooldown.untilMs > Date.now());
  assert.deepEqual(cooled.find(a => a.id === pool[1].id).cooldowns, []);
  const cooledPreview = await previewService({ ...pooledDraft, revision: pooledService.revision });
  assert.deepEqual(cooledPreview.models[0].accounts[0].cooldown, { untilMs: accountCooldown.untilMs, reason: accountCooldown.reason });
  const coolingReply = await callPool(first, "ses_poolcooling");
  assert.equal(seen.at(-1).account, "fixture-pool-b");
  assert.equal(coolingReply.content.filter(part => part.type === "text" && part.text.startsWith("【路由提示】")).length, 1);
  const [poolUser, poolTool, poolResult] = continuation;
  const poolToolWithNotice = { ...poolTool, content: [...poolFallback.content, ...poolTool.content] };
  const pinned = await callPool([poolUser, poolToolWithNotice, poolResult], "ses_poolsame");
  assert(!pinned.content.some(part => part.type === "text" && part.text.startsWith("【路由提示】")));
  assert.equal(seen.at(-1).assistantNotice, false);
  assert.equal(seen.at(-1).account, "fixture-pool-b", "Tool continuation must retain the winning account");
  const compactedTool = [{ role: "user", content: "Compacted confirmed history" }, ...continuation.slice(1)];
  await callPool(compactedTool, "ses_poolsame");
  assert.equal(seen.at(-1).account, "fixture-pool-b", "Compacted tool history inherits the committed account");
  const recorded = await api("/api/requests");
  const recordedRows = Array.isArray(recorded) ? recorded : recorded.items ?? recorded.requests ?? [];
  const standbyRequest = recordedRows.find(r => r.routing?.service?.upstreamAccountLabel === "Account B");
  assert(standbyRequest, "Request history identifies the actual account");
  await api(poolPath, { kind: "set-state", storeId, account: pool[1], expectedState: "enabled", state: "revoked" });
  assert.equal((await previewService({ ...pooledDraft, revision: pooledService.revision })).models[0].accounts[1].state, "revoked");
  const afterRevocation = seen.length;
  await callPool(continuation, "ses_poolsame", 503);
  await callPool(compactedTool, "ses_poolsame", 503);
  await callPool(first, "ses_poolnew", 429);
  assert.equal(seen.length, afterRevocation, "Revocation and disabled cross-model fallback cannot be bypassed");
  const withFallback = await api("/api/model-services", {
    ...pooledDraft, revision: pooledService.revision,
    routingPolicy: {
      enabled: true,
      fallbackAcrossModels: true,
      deadlineSeconds: 5,
      accountPools: [{ modelId: models[0].id, accounts: pool.slice(0, 1) }],
    },
  }, 200, "PUT");
  assert.equal((await callPool(first, "ses_poolwithfallback")).model, sources[1].model);
  const pendingSession = "ses_poolpending";
  const beforePending = seen.length;
  hold = true;
  arrive = () => {};
  release = undefined;
  try {
    await callPool(first, pendingSession, 504);
  } finally {
    hold = false;
    release?.();
  }
  assert.deepEqual(seen.slice(beforePending).map(row => row.model), [sources[1].model]);
  const pending = await detailFor(pendingSession);
  assert.equal(pending.routeTrace.sentUpstream, true);
  assert(pending.routeTrace.decisions.some(
    decision => decision.reason === "upstream-outcome-unconfirmed",
  ));

  const rollbackOperation = {
    operationId: randomUUID(),
    storeId,
    action: {
      kind: "save",
      draft: {
        ...pooledDraft,
        revision: withFallback.revision,
        routingPolicy: { enabled: false },
      },
    },
  };
  const beforeRollback = seen.length;
  await api("/api/model-service-operations", rollbackOperation, 400);
  assert.equal(seen.length, beforeRollback, "A rollback requires current verification of the default connection");
  limitedAccount = undefined;
  const restoredVerification = await api("/api/model-service-members/verify", {
    storeId,
    providerId: poolProvider.id,
    expectedRevision: poolProvider.revision,
    model: poolProvider.model,
  });
  assert.equal(restoredVerification.state, "verified");
  assert.equal(seen.at(-1).account, "fixture-pool-a");
  await api("/api/model-service-operations", { ...rollbackOperation, storeId: randomUUID() }, 409);
  const rollbackReceipt = await api("/api/model-service-operations", rollbackOperation);
  assert.deepEqual(await api("/api/model-service-operations", rollbackOperation), rollbackReceipt);
  const rollbackReceiptPath = `/api/model-service-operations/${rollbackOperation.operationId}?storeId=${storeId}`;
  assert.deepEqual(await api(rollbackReceiptPath), rollbackReceipt);
  const rollback = rollbackReceipt.service;
  assert.equal(rollback.revision, withFallback.revision + 1);
  assert.equal(rollback.routingPolicy.enabled, false);
  assert(!rollback.routingPolicy.accountPools?.length);
  assert.equal(hash(files.opencode), installedPoolHash, "A rollback publication keeps the attached client configuration");
  const beforePinnedRollback = seen.length;
  await callPool(continuation, "ses_poolsame", 503);
  assert.equal(seen.length, beforePinnedRollback, "A rollback cannot redirect a tool continuation from a revoked account");
  await callPool(first, pendingSession, 409);
  assert.equal(seen.length, beforePinnedRollback, "Rollback cannot resend a request whose result is unconfirmed");
  const pendingDetail = await api(`/api/request-detail?id=${encodeURIComponent(pending.request.id)}`);
  assert.deepEqual(pendingDetail.routeTrace, pending.routeTrace);
  await callPool(first, "ses_poolrollback", 429);
  assert.equal(seen.length, beforePinnedRollback, "The fixed service must still honor the default credential's cooldown");
  const afterRollback = (await api(poolPath, { kind: "list" })).accounts.find(a => a.id === pool[0].id);
  assert.deepEqual(afterRollback.cooldowns, [accountCooldown], "Publishing and local rejections must preserve the deadline");
  await new Promise(resolve => setTimeout(resolve, Math.max(0, accountCooldown.untilMs - Date.now()) + 50));
  await assertPool(first, "ses_poolrollback", "a");
  limitedAccountRetryAfter = "60";
  const historicalRequest = await api(`/api/request-detail?id=${encodeURIComponent(standbyRequest.id)}`);
  const standbyAttempt = historicalRequest.request.attempts.find(
    attempt => attempt.routing?.service?.upstreamAccountLabel === "Account B",
  );
  const standbyRoute = standbyAttempt?.routing.service;
  assert(standbyRoute, "Rollback keeps the historical attempt's actual account");
  assert.equal(standbyRoute.serviceVersionId, standbyRequest.routing.service.serviceVersionId);
  assert.notEqual(standbyRoute.serviceVersionId, rollback.versionId);
  const exportQuery = new URLSearchParams({ source: "gateway", session: historicalRequest.request.sessionId });
  const dashboard = await api(`/api/request-dashboard?${exportQuery}`);
  const logicalRequestId = historicalRequest.request.id;
  assert(dashboard.items.some(item => item.id === logicalRequestId));
  const exported = await api(`/api/request-export?snapshot=${encodeURIComponent(dashboard.snapshot)}`);
  assert(exported.csv.includes(logicalRequestId), "The routed request is included in the export");
  const protectedAccountFields = [
    ...pool.map(account => account.id), ...poolCredentials, "fixture-pool-a", "fixture-pool-b",
  ];
  assert(!Object.hasOwn(standbyRequest.routing.service, "upstreamAccount"));
  assert(!Object.hasOwn(standbyRoute, "upstreamAccount"));
  for (const payload of [JSON.stringify(standbyRequest), JSON.stringify(historicalRequest), exported.csv]) {
    for (const value of protectedAccountFields) {
      assert(!payload.includes(value), "Request observation exposed an internal account identifier or credential");
    }
  }
  assert(!exported.csv.includes("Account B"), "CSV contains aggregate attempts without account labels");
  console.log(JSON.stringify({
    accountPoolRollback: true,
    defaultCredentialReused: true,
    defaultCredentialCooldownPreserved: true,
    clientConfigPreserved: true,
    historyPreserved: true,
    unconfirmedOutcomePreserved: true,
    requestObservationRedacted: true,
  }));

  const poolRestore = await api(`/api/provider-switches/${poolConnect.id}/restore`, {});
  await api(`/api/provider-switches/${poolRestore.id}/apply`, {});
  await api(`/api/model-services/${rollback.id}/delete`, { expectedRevision: rollback.revision });

  const workspace = path.join(home, "rules-project");
  const v2Workspace = path.join(home, "z-opencode-v2-only");
  const opencodeData = path.join(home, ".local/share/opencode");
  mkdirSync(workspace);
  mkdirSync(v2Workspace);
  mkdirSync(opencodeData, { recursive: true });
  const native = new DatabaseSync(path.join(opencodeData, "opencode.db"));
  try {
    native.exec(`
      CREATE TABLE session(id TEXT PRIMARY KEY,title TEXT,directory TEXT,version TEXT,time_created INTEGER,time_updated INTEGER);
      CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
      CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
      CREATE TABLE session_v2(id TEXT PRIMARY KEY,directory TEXT NOT NULL);
    `);
    const legacy = native.prepare("INSERT INTO session VALUES(?,?,?,?,?,?)");
    for (const id of ["ses_rulesproject", "ses_rulesmigrated"]) {
      legacy.run(id, "Routing fixture", workspace, "1.18.15", Date.now(), Date.now());
    }
    const current = native.prepare("INSERT INTO session_v2 VALUES(?,?)");
    for (const id of ["ses_rulesv2project", "ses_rulesv2hidden"]) current.run(id, workspace);
    current.run("ses_rulesmigrated", "");
    current.run("ses_rulesv2only", v2Workspace);
    for (let index = 0; index < 42; index++) {
      const extraProject = path.join(home, `fixture-paged-${String(index).padStart(2, "0")}`);
      mkdirSync(extraProject);
      current.run(`ses_project${index}`, extraProject);
    }
  } finally {
    native.close();
  }
  const projectId = "project:" + createHash("sha256").update(workspace).digest("hex");
  const rule = (name, modelId, conditions) => ({ id: randomUUID(), name, modelId, ...conditions });
  const [a, b] = models.map(model => model.id);
  const rules = [
    rule("Project", b, { agent: "opencode", projectId }),
    rule("Different client", b, { agent: "claude", tools: true }),
    rule("Streaming only", b, { agent: "opencode", stream: true }),
    rule("OpenCode tools", b, { agent: "opencode", tools: true, stream: false, minInputTokens: 2048 }),
    rule("Other OpenCode tools", a, { agent: "opencode", tools: true }),
    rule("OpenCode CLI", b, { agent: "opencode-cli" }),
    rule("OpenCode Desktop", b, { agent: "opencode-desktop" }),
  ];
  const ruleDraft = {
    ...draft, id: randomUUID(), name: "Conditional routing",
    routingPolicy: { enabled: true, rules },
  };
  await api("/api/model-services", {
    ...ruleDraft,
    routingPolicy: { enabled: true, rules: [rule("Unknown target", randomUUID(), { agent: "opencode" })] },
  }, 400, "PUT");
  let conditioned = await api("/api/model-services", ruleDraft, 200, "PUT");
  assert.deepEqual(conditioned.routingPolicy.rules, rules);
  const conditionConfig = (await api("/api/provider-status")).configs.find(c => c.agentId === "opencode");
  const conditionConnection = await api("/api/model-service-connections", {
    agentId: "opencode", expectedRevision: conditionConfig.revision,
    serviceId: conditioned.id, expectedServiceRevision: conditioned.revision,
  });
  await api(`/api/provider-switches/${conditionConnection.id}/apply`, {});
  const conditionFile = JSON.parse(readFileSync(files.opencode));
  const conditionOptions = conditionFile.provider[conditionFile.model.split("/")[0]].options;
  const callRule = async (session, options = {}, expected = 200, headers = {}) => {
    const r = await fetch(conditionOptions.baseURL + "/messages", {
      method: "POST",
      headers: { Authorization: `Bearer ${conditionOptions.apiKey}`, "Content-Type": "application/json", "x-opencode-session": session, ...headers },
      body: JSON.stringify({ model: "local-agent-default", max_tokens: 30, messages: first, ...options }),
      signal: AbortSignal.timeout(20000),
    });
    const body = await r.text();
    assert.equal(r.status, expected, body);
    if (expected !== 200) return;
    if (options.stream) {
      assert.match(body, /"text":"OK","type":"text_delta"/);
      return seen.at(-1).model;
    }
    return JSON.parse(body).model;
  };
  const conditionFileHash = hash(files.opencode);
  for (const disabled of ["desktop", "cli", null]) {
    await changeBuiltin(disabled ? [`opencode-${disabled}`] : []);
    for (const [client, name] of [["cli", "CLI"], ["desktop", "Desktop"]]) {
      const enabled = client !== disabled;
      const session = "ses_" + randomUUID().replaceAll("-", "");
      const before = seen.length;
      const expectedModel = enabled ? "b" : "a";
      assert.equal(await callRule(session, {}, 200, { "User-Agent": `opencode/latest/2.2.7/${client}` }), expectedModel);
      assert.deepEqual(seen.slice(before).map(item => item.model), [expectedModel]);
      const detail = await detailFor(session, 200);
      const attempt = detail.request.attempts[0];
      assert.equal(attempt.agentProductId ?? null, enabled ? `opencode-${client}` : null);
      assert.equal(attempt.routing.service.routingRule?.name ?? null, enabled ? `OpenCode ${name}` : null);
    }
    assert.equal(hash(files.opencode), conditionFileHash, "Changing a product plugin must preserve the shared OpenCode connection");
  }
  console.log(JSON.stringify({ separateOpenCodeProducts: true, independentPluginLifecycle: true, sharedConfigPreserved: true }));
  assert.equal(await callRule("ses_rulesproject"), "a", "A request's session claim cannot authorize project reads");
  assert.equal(await callRule("ses_rulesv2project"), "a", "A 2.x session claim also requires source consent");
  const sendsBeforeTitle = seen.length;
  for (const session of ["ses_rulesv2project", "ses_rulesv2hidden"]) {
    await callRule(session, { max_tokens: 32000, messages: [
      { role: "user", content: "Isolated OpenCode title generation" },
    ] }, 400);
  }
  assert.equal(seen.length, sendsBeforeTitle, "Rejected auxiliary output requests must send nothing");
  const choicesPath = "/api/model-services/routing-projects";
  const projectChoices = (query = {}, status = 200) => api(`${choicesPath}?${new URLSearchParams(query)}`, undefined, status);
  const noProjectAuth = await fetch(base + choicesPath);
  assert.equal(noProjectAuth.status, 401);
  await noProjectAuth.text();
  assert.deepEqual(await projectChoices(), { items: [], nextCursor: null, selected: null });
  await api("/api/session-sources", { agentId: "opencode", enabled: true }, 200, "PUT");
  await api("/api/sessions/refresh", {});
  let scanned = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    const sessionList = await api("/api/sessions?agent=opencode");
    if (sessionList.items.some(item => item.nativeId === "ses_rulesproject" && item.projectId === projectId)) {
      scanned = true;
      break;
    }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert(scanned, "The authorized native index did not publish the fixture project");
  const firstProjects = await projectChoices();
  assert.equal(firstProjects.items.length, 40);
  assert(firstProjects.nextCursor);
  const nextProjects = await projectChoices({ cursor: firstProjects.nextCursor });
  assert.equal(nextProjects.nextCursor, null);
  assert.equal(new Set([...firstProjects.items, ...nextProjects.items].map(project => project.id)).size, 44);
  const v2Choices = await projectChoices({ query: "Z-OPENCODE-V2-ONLY" });
  assert.equal(v2Choices.items.length, 1);
  const [v2Choice] = v2Choices.items;
  assert.deepEqual(v2Choice, { id: "project:" + createHash("sha256").update(v2Workspace).digest("hex"), name: "z-opencode-v2-only", path: v2Workspace, projectKind: "temporary" });
  assert.deepEqual(await projectChoices({ query: "no match", selectedId: v2Choice.id }), { items: [], nextCursor: null, selected: v2Choice });
  await projectChoices({ query: "different query", cursor: firstProjects.nextCursor }, 409);
  rules.push(rule("OpenCode 2.x selected project", b, { agent: "opencode", projectId: v2Choice.id }));
  conditioned = await api("/api/model-services", { ...ruleDraft, revision: conditioned.revision, routingPolicy: { enabled: true, rules } }, 200, "PUT");
  assert.equal(await callRule("ses_rulesv2only"), "b", "The selected 2.x-only project can be published and route a new turn");

  const projectsPlugin = await api("/api/builtin-plugins");
  await api("/api/builtin-plugins/changes", {
    operationId: randomUUID(), storeId: projectsPlugin.storeId, expectedRevision: projectsPlugin.revision,
    expectedGeneration: projectsPlugin.host.generation, expectedImpact: projectsPlugin.impact,
    composition: { ...projectsPlugin.composition, optedInPluginIds: [...new Set([...(projectsPlugin.composition.optedInPluginIds ?? []), "builtin.projects"])].sort() },
  });
  assert.equal((await api("/api/projects?scope=all&query=z-opencode-v2-only")).items.length, 0, "Routing choices are not transcript statistics");
  await api("/api/sessions/refresh", {});
  for (let attempt = 0; attempt < 100; attempt++) {
    if ((await projectChoices({ selectedId: projectId })).selected?.id === projectId) break;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.equal(await callRule("ses_rulesproject"), "a", "An existing turn keeps its original route after indexing");
  const nextProjectTurn = [
    ...first, { role: "assistant", content: "OK" }, { role: "user", content: "project new turn" },
  ];
  assert.equal(await callRule("ses_rulesproject", { messages: nextProjectTurn }), "b");
  assert.equal(await callRule("ses_rulesv2project", { messages: nextProjectTurn }), "b", "Authorized 2.x project metadata can route a new turn");
  assert.equal(await callRule("ses_rulesmigrated"), "a", "An older 1.x project cannot override an unknown 2.x directory for the same session");
  await api("/api/project-visibility", { projectId, hidden: true }, 200, "PUT");
  assert.equal((await projectChoices({ selectedId: projectId })).selected, null);
  await projectChoices({ cursor: firstProjects.nextCursor }, 409);
  assert.equal(await callRule("ses_rulesv2hidden"), "a", "A hidden 2.x project cannot match a rule");
  await api("/api/project-visibility", { projectId, hidden: false }, 200, "PUT");
  assert.equal(await callRule("ses_rulesv2hidden", { messages: nextProjectTurn }), "b");
  assert.equal(await callRule("ses_unknownproject"), "a");
  assert.equal(await callRule("ses_rulesstream", { stream: true }), "b");
  const tools = [{ name: "read", input_schema: { type: "object" } }];
  assert.equal(await callRule("ses_rulesshort", { tools }), "a", "Short input does not match the minimum");
  const longTurn = [{ role: "user", content: "routing context ".repeat(800) }];
  const toolOptions = { tools, messages: longTurn };
  assert.equal(await callRule("ses_rulestools", toolOptions), "b", "The first matching rule wins");
  assert.equal(await callRule("ses_rulesexplicit", { ...toolOptions, model: `local-agent-model-${a}` }), "a");
  assert.equal(await callRule("ses_rulestools", { messages: [...longTurn, ...continuation.slice(1)] }), "b", "Tool history stays with the original rule target");
  const observed = await api("/api/requests");
  assert((observed.items ?? observed).some(row =>
    row.routing?.service?.routingRule?.name === "OpenCode tools" &&
    row.routing.service.modelId === b && !row.routing.service.fallbackReason));
  await api("/api/session-sources", { agentId: "opencode", enabled: false }, 200, "PUT");
  assert.deepEqual(await projectChoices({ selectedId: v2Choice.id }), { items: [], nextCursor: null, selected: null });
  const revokedProjectTurn = [
    ...nextProjectTurn, { role: "assistant", content: "OK" }, { role: "user", content: "revoked project" },
  ];
  for (const session of ["ses_rulesproject", "ses_rulesv2project"]) {
    assert.equal(await callRule(session, { messages: revokedProjectTurn }), "a");
  }
  conditioned = await api("/api/model-services", {
    ...ruleDraft, revision: conditioned.revision,
    routingPolicy: { enabled: true, rules: [rules[4], ...rules.slice(0, 4), ...rules.slice(5)] },
  }, 200, "PUT");
  assert.equal(await callRule("ses_rulestools", toolOptions), "b", "Replay keeps the original rules and target");
  assert.equal(await callRule("ses_rulesreordered", toolOptions), "a", "New turns follow the published rule order");
  const intentRules = [
    rule("Static order", a, { tools: true }),
    rule("Debug routing", b, { agent: "opencode", intent: "排查错误" }),
    rule("General routing", a, { agent: "opencode" }),
  ];
  const intentDraft = { ...ruleDraft, routingPolicy: { enabled: true, rules: intentRules } };
  conditioned = await api("/api/model-services", { ...intentDraft, revision: conditioned.revision }, 200, "PUT");
  assert.equal(await callRule("ses_intentoff"), "a");
  assert.equal(classifierCalls.length, 0);
  conditioned = await api("/api/model-services", {
    ...intentDraft, revision: conditioned.revision,
    routingPolicy: { ...intentDraft.routingPolicy, intentClassifier: { modelId: a } },
  }, 200, "PUT");
  const intentHistory = [
    { role: "user", content: "private earlier history" },
    { role: "assistant", content: "private earlier assistant" },
    { role: "user", content: [
      { type: "text", text: "<system-reminder>private client context</system-reminder>" },
      { type: "text", text: "Debug this request with password=private-current-key" },
    ] },
  ];
  const intentOptions = { messages: intentHistory, system: "private original system prompt" };
  assert.equal(await callRule("ses_intentexplicit", { ...intentOptions, model: `local-agent-model-${a}` }), "a");
  assert.equal(await callRule("ses_intentstatic", { ...intentOptions, tools }), "a", "An earlier static rule does not call the classifier");
  assert.equal(classifierCalls.length, 0);
  assert.equal(await callRule("ses_intentmatched", intentOptions), "b");
  assert.equal(classifierCalls.length, 1);
  const classifier = classifierCalls[0];
  const sent = JSON.stringify(classifier);
  assert.equal(classifier.model, "a");
  assert.equal(classifier.tools, undefined);
  assert(sent.includes("Debug this request") && sent.includes("[已隐藏凭据]"));
  for (const privateText of ["private-current-key", "private earlier", "private client context", "private original system prompt"])
    assert(!sent.includes(privateText), privateText);
  assert.equal(await callRule("ses_intentmatched", intentOptions), "b");
  assert.equal(await callRule("ses_intentmatched", { messages: [...intentHistory, ...continuation.slice(1)] }), "b");
  assert.equal(classifierCalls.length, 1, "Replay and tool continuation must keep the original intent");
  const intentObserved = await api("/api/requests");
  const intentRows = intentObserved.items ?? intentObserved;
  assert(intentRows.some(row => row.routing?.service?.routingRule?.name === "Debug routing" &&
    row.routing.service.routingIntent?.matched === "排查错误" && row.routing.service.modelId === b));
  assert(intentRows.some(row => row.agentId === "model-router"), "Classifier usage is recorded separately");
  classifierReply = "invalid classification";
  assert.equal(await callRule("ses_intentunknown"), "a");
  classifierReply = "timeout";
  assert.equal(await callRule("ses_intenttimeout"), "a");
  assert.equal(classifierCalls.length, 3);
  const planFailure = new DatabaseSync(path.join(directory, "app/local-agent.sqlite3"));
  try {
    planFailure.exec(`CREATE TRIGGER fixture_intent_plan_failure BEFORE INSERT ON service_route_dispatch
      WHEN NEW.session='opencode:ses_intentBeforePlan' BEGIN SELECT RAISE(FAIL,'fixture plan unavailable'); END`);
    classifierReply = "1";
    await callRule("ses_intentBeforePlan", intentOptions, 503);
    assert.equal(classifierCalls.length, 4);
  } finally {
    planFailure.exec("DROP TRIGGER IF EXISTS fixture_intent_plan_failure");
    planFailure.close();
  }
  assert.equal(await callRule("ses_intentBeforePlan", intentOptions), "b");
  assert.equal(classifierCalls.length, 4, "A confirmed classifier result must survive a failed answer plan");
  const intentFailure = new DatabaseSync(path.join(directory, "app/local-agent.sqlite3"));
  try {
    intentFailure.exec(`CREATE TRIGGER fixture_intent_result_failure BEFORE UPDATE OF outcome ON service_route_intents
      WHEN NEW.session='opencode:ses_intentUnconfirmed' BEGIN SELECT RAISE(FAIL,'fixture result unavailable'); END;
      CREATE TRIGGER fixture_intent_plan_failure BEFORE INSERT ON service_route_dispatch
      WHEN NEW.session='opencode:ses_intentUnconfirmed' BEGIN SELECT RAISE(FAIL,'fixture plan unavailable'); END`);
    await callRule("ses_intentUnconfirmed", intentOptions, 503);
    assert.equal(classifierCalls.length, 5);
  } finally {
    intentFailure.exec("DROP TRIGGER IF EXISTS fixture_intent_result_failure; DROP TRIGGER IF EXISTS fixture_intent_plan_failure");
    intentFailure.close();
  }
  assert.equal(await callRule("ses_intentUnconfirmed", intentOptions), "a");
  assert.equal(classifierCalls.length, 5, "An unconfirmed classifier result must not start another classifier request");
  const conditionRestore = await api(`/api/provider-switches/${conditionConnection.id}/restore`, {});
  await api(`/api/provider-switches/${conditionRestore.id}/apply`, {});
  await api(`/api/model-services/${conditioned.id}/delete`, { expectedRevision: conditioned.revision });

  limitedAccount = undefined;
  const nestedAccounts = [];
  for (const [key, label] of [["fixture-nested-a", "Nested A"], ["fixture-nested-b", "Nested B"]]) {
    const { credentialRef } = await api("/api/provider-credentials", { label, secret: key });
    const id = randomUUID();
    const saved = await api(poolPath, {
      kind: "save", storeId,
      account: { id, expectedRevision: 0, providerRevision: poolProvider.revision, label, credentialRef },
    });
    const upstreamAccount = { id, revision: saved.accounts.find(account => account.id === id).revision };
    nestedAccounts.push(upstreamAccount);
    const verification = await api("/api/model-service-members/verify", {
      storeId, providerId: poolProvider.id, expectedRevision: poolProvider.revision, model: poolProvider.model, upstreamAccount,
    });
    assert.equal(verification.state, "verified");
  }
  const publish = (value, expected = 200) => api("/api/model-services", value, expected, "PUT");
  const leafModel = { ...models[0], id: randomUUID(), providerId: poolProvider.id };
  const leafDraft = {
    id: randomUUID(), name: "Nested leaf", revision: 0, defaultModelId: leafModel.id, models: [leafModel],
    routingPolicy: {
      enabled: true,
      accountPools: [{ modelId: leafModel.id, accounts: nestedAccounts, affinity: "strict" }],
      limits: [{ modelId: leafModel.id, maxConcurrent: 1, requestsPerMinute: 0 }],
    },
  };
  let leaf = await publish(leafDraft);
  const memberFor = (service) => ({
    id: randomUUID(), name: service.name, child: { serviceId: service.id, versionId: service.versionId },
    capabilities: models[0].capabilities,
  });
  const serviceFor = (name, group, rest = []) => ({
    id: randomUUID(), name, revision: 0, defaultModelId: group.id, models: [group, ...rest],
    routingPolicy: { enabled: true },
  });
  const middleGroup = memberFor(leaf);
  const middleDraft = serviceFor("Nested middle", middleGroup, [models[1], models[0]]);
  middleDraft.routingPolicy.rules = [
    rule("Nested intent", b, { agent: "opencode", stream: true, intent: "解释概念" }),
    rule("Nested tools", b, { agent: "opencode", tools: true }),
  ];
  middleDraft.routingPolicy.intentClassifier = { modelId: a };
  const middle = await publish(middleDraft);
  const rootGroup = memberFor(middle);
  const rootDraft = serviceFor("Nested root", rootGroup);
  let root = await publish(rootDraft);
  assert.deepEqual(root.models[0].child, rootGroup.child);
  await publish(serviceFor("Too deep", memberFor(root)), 400);
  const loop = memberFor(root);
  await publish({ ...leafDraft, revision: leaf.revision, defaultModelId: loop.id, models: [loop], routingPolicy: { enabled: true } }, 400);
  await api(`/api/model-services/${leaf.id}/delete`, { expectedRevision: leaf.revision }, 409);
  const verificationFile = path.join(directory, "app", "provider-configs", "verifications.json");
  const freshVerifications = readFileSync(verificationFile);
  const expiredVerifications = JSON.parse(freshVerifications).map(verification => ({
    ...verification, checkedAt: "2000-01-01T00:00:00Z",
  }));
  writeFileSync(verificationFile, JSON.stringify(expiredVerifications));
  const expiredA = (await api(poolPath, { kind: "list" })).accounts.find(a => a.id === pool[0].id);
  assert.equal(expiredA.verifications.find(v => v.model === poolProvider.model).state, "expired");
  const expiredPreview = await previewService({ ...leafDraft, revision: leaf.revision });
  assert.equal(expiredPreview.models[0].accounts[0].verification, "expired");
  const beforeAttach = seen.length;
  const nestedConfig = (await api("/api/provider-status")).configs.find(config => config.agentId === "opencode");
  const nestedConnect = await api("/api/model-service-connections", {
    agentId: "opencode", expectedRevision: nestedConfig.revision, serviceId: root.id, expectedServiceRevision: root.revision,
  });
  await api(`/api/provider-switches/${nestedConnect.id}/apply`, {});
  assert.equal(seen.length, beforeAttach, "Attaching a published child doesn't require another paid probe");
  writeFileSync(verificationFile, freshVerifications);
  const nestedFile = JSON.parse(readFileSync(files.opencode));
  const nestedOptions = nestedFile.provider[nestedFile.model.split("/")[0]].options;
  const callNested = (session, messages = first, model = "local-agent-default", options = {}) => fetch(nestedOptions.baseURL + "/messages", {
    method: "POST",
    headers: { Authorization: `Bearer ${nestedOptions.apiKey}`, "Content-Type": "application/json", "x-opencode-session": session },
    body: JSON.stringify({ model, max_tokens: 30, messages, ...options }), signal: AbortSignal.timeout(20000),
  });
  const nestedStrictSession = "ses_nestedStrict";
  const strictSeed = await callNested(nestedStrictSession);
  assert.equal(strictSeed.status, 200, await strictSeed.text());
  assert.equal(seen.at(-1).account, "fixture-nested-a");
  limitedAccount = "fixture-nested-a";
  limitedAccountRetryAfter = "1";
  const beforeStrictNested = seen.length;
  const strictRate = await callNested(nestedStrictSession, second);
  assert.equal(strictRate.status, 429, await strictRate.text());
  assert.equal(strictRate.headers.get("retry-after"), "1");
  assert.deepEqual(seen.slice(beforeStrictNested).map(call => call.account), ["fixture-nested-a"],
    "A strict child account's 429 must not send the parent's fallback");
  const strictSibling = decision => decision.reason === "strict-affinity" &&
    decision.path.length === 2 && decision.path.at(-1).modelId === b;
  const strictTrace = await detailFor(nestedStrictSession, 429);
  assert(strictTrace.routeTrace.decisions.some(strictSibling));
  const readNestedAccount = async (account = nestedAccounts[0]) =>
    (await api(poolPath, { kind: "list" })).accounts.find(row => row.id === account.id);
  const activeNested = await readNestedAccount();
  const [nestedCooldown] = activeNested.cooldowns;
  assert.equal(nestedCooldown.model, null);
  assert.equal(nestedCooldown.reason, "upstream_rate_limit");
  assert.deepEqual(activeNested.recoveries, []);
  limitedAccount = undefined;
  await new Promise(resolve => setTimeout(resolve, Math.max(0, nestedCooldown.untilMs - Date.now()) + 50));
  const waitingNested = await readNestedAccount();
  assert.deepEqual(waitingNested.cooldowns, []);
  assert.deepEqual(waitingNested.recoveries, [{ ...nestedCooldown, state: "waitingForProbe" }]);
  const strictCapacitySession = "ses_nestedStrictCapacity";
  hold = true;
  const recoveryArrived = new Promise(resolve => { arrive = resolve; });
  const seedResponse = callNested(strictCapacitySession);
  await Promise.race([recoveryArrived, seedResponse.then(() => assert.fail("Recovery response was not held"))]);
  try {
    const probingNested = await readNestedAccount();
    assert.deepEqual(probingNested.cooldowns, []);
    assert.deepEqual(probingNested.recoveries, [{ ...nestedCooldown, state: "probing" }]);
    assert.deepEqual((await readNestedAccount(nestedAccounts[1])).recoveries, []);
  } finally {
    release();
  }
  const capacitySeed = await seedResponse;
  assert.equal(capacitySeed.status, 200, await capacitySeed.text());
  assert.deepEqual((await readNestedAccount()).recoveries, []);
  assert.equal(seen.at(-1).account, "fixture-nested-a");
  hold = true;
  const capacityOccupied = new Promise(resolve => { arrive = resolve; });
  const capacityHolder = callNested("ses_nestedCapacityHolder");
  await capacityOccupied;
  try {
    const beforeCapacity = seen.length;
    const strictCapacity = await callNested(strictCapacitySession, second);
    assert.equal(strictCapacity.status, 429, await strictCapacity.text());
    assert.equal(seen.length, beforeCapacity, "A strict child at capacity must not send to a sibling");
    const capacityTrace = await detailFor(strictCapacitySession, 429);
    assert(capacityTrace.routeTrace.decisions.some(decision => decision.reason === "configured-rate-limit"));
    assert(capacityTrace.routeTrace.decisions.some(strictSibling));
  } finally {
    release();
    const held = await capacityHolder;
    assert.equal(held.status, 200, await held.text());
  }
  limitedAccount = "fixture-nested-a";
  limitedAccountRetryAfter = "1";
  const nestedRecoverySession = "ses_nestedRecovery";
  const recoveryDb = new DatabaseSync(path.join(directory, "app/local-agent.sqlite3"));
  try {
    recoveryDb.exec(`CREATE TRIGGER fixture_nested_backup_interrupt BEFORE UPDATE ON service_route_attempts
      WHEN NEW.session='opencode:${nestedRecoverySession}' AND EXISTS(
        SELECT 1 FROM service_route_attempts first WHERE first.session=NEW.session
        AND first.id<NEW.id AND first.outcome='rate_limited')
      BEGIN SELECT RAISE(FAIL,'fixture nested backup interrupted'); END`);
    const before = seen.length;
    const interrupted = await callNested(nestedRecoverySession);
    assert.equal(interrupted.status, 503, await interrupted.text());
    assert.deepEqual(seen.slice(before).map(call => call.account), ["fixture-nested-a"]);
    const saved = recoveryDb.prepare(`SELECT json_extract(value,'$.fallback') AS checkpoint
      FROM service_route_dispatch WHERE session=?`).get(`opencode:${nestedRecoverySession}`);
    const checkpoint = JSON.parse(saved.checkpoint);
    assert.deepEqual(checkpoint.progress.frames.map(frame => frame.attempts), [1, 1, 1]);
    // Older releases captured this same parent continuation even after a
    // pinned leaf failed. The pinned leaf had no remaining account.
    const strict = recoveryDb.prepare(`SELECT d.binding,d.session,d.turn,d.value,a.id
      FROM service_route_dispatch d JOIN service_route_attempts a USING(binding,session,turn)
      WHERE d.session=? AND a.outcome='rate_limited'`).get(`opencode:${nestedStrictSession}`);
    const value = JSON.parse(strict.value);
    assert.equal(value.fallback, undefined, "A strict failure must not create a parent checkpoint");
    checkpoint.through = strict.id;
    checkpoint.progress.frames.at(-1).remaining = [];
    value.fallback = checkpoint;
    assert.equal(recoveryDb.prepare(`UPDATE service_route_dispatch SET state='pending',value=?
      WHERE binding=? AND session=? AND turn=? AND state='failed'`)
      .run(JSON.stringify(value), strict.binding, strict.session, strict.turn).changes, 1);
  } finally {
    recoveryDb.exec("DROP TRIGGER IF EXISTS fixture_nested_backup_interrupt");
    recoveryDb.close();
  }
  await new Promise(resolve => setTimeout(resolve, 1100));
  limitedAccount = undefined;
  const beforeRecovery = seen.length;
  const nestedRecovery = await callNested(nestedRecoverySession);
  assert.equal(nestedRecovery.status, 200, await nestedRecovery.text());
  assert.deepEqual(seen.slice(beforeRecovery).map(call => call.account), ["fixture-nested-b"]);
  const beforeLegacyStrict = seen.length;
  const legacyStrict = await callNested(nestedStrictSession, second);
  const legacyStrictBody = await legacyStrict.text();
  assert.equal(legacyStrict.status, 409, legacyStrictBody);
  assert.match(legacyStrictBody, /原轮次固定的上游账号已失败/);
  assert.equal(seen.length, beforeLegacyStrict, "A legacy parent checkpoint cannot bypass a failed strict child");
  console.log(JSON.stringify({ nestedStrictRatePreserved: true, nestedStrictCapacityStopsBeforeSend: true, legacyStrictFallbackBlocked: true }));
  limitedAccount = "fixture-nested-a";
  limitedAccountRetryAfter = "60";
  const beforeNested = seen.length;
  const nestedAnswer = await callNested("ses_nestedsame");
  assert.equal(nestedAnswer.status, 200, await nestedAnswer.text());
  assert.deepEqual(seen.slice(beforeNested).map(call => call.account), ["fixture-nested-a", "fixture-nested-b"],
    "Child account fallback runs before advancing to the parent's next member");
  const oldLeafVersion = leaf.versionId;
  leaf = await publish({ ...leafDraft, revision: leaf.revision, defaultModelId: models[1].id, models: [models[1]], routingPolicy: { enabled: true } });
  const beforePinnedPreview = seen.length;
  const pinnedPreview = await previewService({ ...rootDraft, revision: root.revision });
  const pinnedLeaf = pinnedPreview.models[0].child.models.find(m => m.id === middleGroup.id).child;
  assert.equal(pinnedLeaf.versionId, oldLeafVersion);
  assert.equal(pinnedLeaf.models[0].accounts[0].label, "Nested A");
  assert.equal(pinnedLeaf.models[0].accounts[0].verifyOnPublish, false);
  assert.equal(seen.length, beforePinnedPreview);
  console.log(JSON.stringify({ publicationPreview: true, noProbeOrMutation: true, scopedStatusAndReasons: true, pinnedHistoricalChildren: true }));
  const nestedFollowup = await callNested("ses_nestedsame", continuation);
  assert.equal(nestedFollowup.status, 200, await nestedFollowup.text());
  assert.equal(seen.at(-1).account, "fixture-nested-b", "Tool continuation retains the winning child account");
  const nestedExplicit = await callNested("ses_nestednew", first, `local-agent-model-${rootGroup.id}`);
  assert.equal(nestedExplicit.status, 200, await nestedExplicit.text());
  assert.equal(seen.at(-1).account, "fixture-nested-b", "Explicit group aliases still use the pinned child publication");
  const nestedRule = await callNested("ses_nestedrule", first, `local-agent-model-${rootGroup.id}`, toolOptions);
  assert.equal(nestedRule.status, 200);
  assert.equal((await nestedRule.json()).model, "b", "An explicit parent alias still applies the child's rules");
  const nestedObserved = await api("/api/requests");
  assert((nestedObserved.items ?? nestedObserved).some(row => {
    const service = row.routing?.service;
    return service?.serviceId === root.id && service.upstreamAccountLabel === "Nested B" &&
      service.nestedRoute?.length === 2 && service.nestedRoute[1].versionId === oldLeafVersion;
  }), "Request history records both nested hops and the actual account");
  assert((nestedObserved.items ?? nestedObserved).some(row =>
    row.routing?.service?.nestedRoute?.[0].routingRule?.name === "Nested tools"
  ), "Request history records the child rule that was matched");
  classifierReply = "1";
  const beforeNestedIntent = classifierCalls.length;
  const childIntent = await callNested("ses_nestedintent", first, "local-agent-default", { stream: true });
  assert.equal(childIntent.status, 200, await childIntent.text());
  assert.equal(seen.at(-1).model, "b", "The first eligible child can classify and route");
  assert.equal(classifierCalls.length, beforeNestedIntent + 1);
  const repeatedIntent = await callNested("ses_nestedintent", first, "local-agent-default", { stream: true });
  assert.equal(repeatedIntent.status, 200, await repeatedIntent.text());
  assert.equal(classifierCalls.length, beforeNestedIntent + 1, "Replaying the turn must not classify again");
  const childPlanFailure = new DatabaseSync(path.join(directory, "app/local-agent.sqlite3"));
  try {
    childPlanFailure.exec(`CREATE TRIGGER fixture_child_plan_failure BEFORE UPDATE OF value ON service_route_dispatch
      WHEN NEW.session='opencode:ses_childIntentBeforePlan' BEGIN SELECT RAISE(FAIL,'fixture child plan unavailable'); END`);
    const interrupted = await callNested("ses_childIntentBeforePlan", first, "local-agent-default", { stream: true });
    assert.equal(interrupted.status, 503, await interrupted.text());
    assert.equal(classifierCalls.length, beforeNestedIntent + 2);
  } finally {
    childPlanFailure.exec(`DROP TRIGGER IF EXISTS fixture_child_plan_failure;
      UPDATE service_route_dispatch SET retry_at=0 WHERE session='opencode:ses_childIntentBeforePlan'`);
    childPlanFailure.close();
  }
  const resumedChild = await callNested("ses_childIntentBeforePlan", first, "local-agent-default", { stream: true });
  assert.equal(resumedChild.status, 200, await resumedChild.text());
  assert.equal(seen.at(-1).model, "b", "A recovered child plan uses its confirmed classification");
  assert.equal(classifierCalls.length, beforeNestedIntent + 2);
  root = await publish({
    ...rootDraft, revision: root.revision, models: [rootGroup, models[0]],
    routingPolicy: {
      enabled: true, intentClassifier: { modelId: a },
      rules: [rule("Root intent", rootGroup.id, { agent: "opencode", stream: true, intent: "进入子组" })],
    },
  });
  const rootIntent = await callNested("ses_rootintent", first, "local-agent-default", { stream: true });
  assert.equal(rootIntent.status, 200, await rootIntent.text());
  assert.equal(classifierCalls.length, beforeNestedIntent + 3, "Parent and children share one classifier call per turn");
  assert.equal(seen.at(-1).account, "fixture-nested-b", "A child without a classifier call still routes by its normal policy");
  const explicitIntent = await callNested("ses_explicitintent", first, `local-agent-model-${rootGroup.id}`, { stream: true });
  assert.equal(explicitIntent.status, 200, await explicitIntent.text());
  assert.equal(classifierCalls.length, beforeNestedIntent + 3, "Explicit aliases never send an intent excerpt upstream");
  root = await publish({ ...rootDraft, revision: root.revision, routingPolicy: { enabled: true, maxAttempts: 1 } });
  limitedAccount = "fixture-nested-b";
  const beforeCap = seen.length;
  const capped = await callNested("ses_nestedcap");
  assert.equal(capped.status, 429, await capped.text());
  assert.deepEqual(seen.slice(beforeCap).map(call => call.account), ["fixture-nested-b"],
    "Parent attempt limits count sends in nested services");
  root = await publish({ ...rootDraft, revision: root.revision });
  const afterCap = await callNested("ses_nestedfallback");
  assert.equal(afterCap.status, 200);
  assert.equal((await afterCap.json()).model, "b", "An exhausted child advances to the next parent member");
  limitedAccount = undefined;
  const nestedRestore = await api(`/api/provider-switches/${nestedConnect.id}/restore`, {});
  await api(`/api/provider-switches/${nestedRestore.id}/apply`, {});
  for (const published of [root, middle, leaf]) {
    await api(`/api/model-services/${published.id}/delete`, { expectedRevision: published.revision });
  }

  const duplicateKey = "fixture-duplicate-target";
  const { credentialRef: duplicateCredential } = await api("/api/provider-credentials", {
    label: "Shared target", secret: duplicateKey,
  });
  const duplicateProvider = { ...sources[0], id: `custom-${randomUUID()}`, revision: 0, credentialRef: duplicateCredential };
  await api("/api/providers", duplicateProvider, 200, "PUT");
  assert.equal((await api(`/api/providers/${duplicateProvider.id}/verify`, { expectedRevision: 1, model: "a" })).state, "verified");
  const duplicateAccountId = randomUUID();
  const duplicateAccounts = await api(`/api/provider-upstream-accounts/${duplicateProvider.id}`, {
    kind: "save", storeId,
    account: { id: duplicateAccountId, expectedRevision: 0, providerRevision: 1, label: "Shared target", credentialRef: duplicateCredential },
  });
  const duplicateAccount = { id: duplicateAccountId, revision: duplicateAccounts.accounts[0].revision };
  assert.equal((await api("/api/model-service-members/verify", {
    storeId, providerId: duplicateProvider.id, expectedRevision: 1, model: "a", upstreamAccount: duplicateAccount,
  })).state, "verified");
  const duplicateMember = { ...models[0], id: randomUUID(), providerId: duplicateProvider.id };
  const duplicateChild = await publish({
    ...serviceFor("Duplicate child", duplicateMember, [models[0]]),
    routingPolicy: {
      enabled: true,
      accountPools: [{ modelId: duplicateMember.id, accounts: [duplicateAccount] }],
      intentClassifier: { modelId: a },
      rules: [rule("Wait past the first cooldown", duplicateMember.id, { agent: "opencode", intent: "核对重复候选" })],
    },
  });
  const duplicateRoot = await publish({
    ...serviceFor("Duplicate root", duplicateMember, [memberFor(duplicateChild), models[0]]),
    routingPolicy: { enabled: true, fallbackOnFailure: false },
  });
  const duplicateConfig = (await api("/api/provider-status")).configs.find(config => config.agentId === "opencode");
  const duplicateConnect = await api("/api/model-service-connections", {
    agentId: "opencode", expectedRevision: duplicateConfig.revision, serviceId: duplicateRoot.id, expectedServiceRevision: duplicateRoot.revision,
  });
  await api(`/api/provider-switches/${duplicateConnect.id}/apply`, {});
  const duplicateFile = JSON.parse(readFileSync(files.opencode));
  const duplicateOptions = duplicateFile.provider[duplicateFile.model.split("/")[0]].options;
  limitedAccount = duplicateKey;
  limitedAccountRetryAfter = "1";
  classifierReply = "timeout";
  const beforeDuplicate = seen.length;
  const duplicateAnswer = await fetch(duplicateOptions.baseURL + "/messages", {
    method: "POST",
    headers: { Authorization: `Bearer ${duplicateOptions.apiKey}`, "Content-Type": "application/json", "x-opencode-session": "ses_nestedduplicate" },
    body: JSON.stringify({ model: "local-agent-default", max_tokens: 30, messages: first }),
    signal: AbortSignal.timeout(20000),
  });
  assert.equal(duplicateAnswer.status, 200, await duplicateAnswer.text());
  assert.equal(seen.slice(beforeDuplicate).filter(call => call.account === duplicateKey).length, 1,
    "A physical target reached through both a direct member and a child account pool can only send once per dispatch");
  const duplicateTrace = await detailFor("ses_nestedduplicate");
  assert(duplicateTrace.routeTrace.decisions.some(d =>
    d.reason === "duplicate-target" && d.path.length === 2 && d.path.every(step => step.versionId),
  ));
  limitedAccount = undefined;
  const duplicateRestore = await api(`/api/provider-switches/${duplicateConnect.id}/restore`, {});
  await api(`/api/provider-switches/${duplicateRestore.id}/apply`, {});
  for (const published of [duplicateRoot, duplicateChild]) {
    await api(`/api/model-services/${published.id}/delete`, { expectedRevision: published.revision });
  }
  assert.equal((await api("/api/model-services")).services.length, 0);
  console.log(JSON.stringify({ nestedRouting: true, nestedAccountFallback: true, interruptedNestedFallback: true, pinnedVersionsAndToolContinuation: true, boundedParentAttempts: true, cyclicAndDeepReferencesRejected: true, singleIntentAcrossTree: true, deduplicatesAcrossTree: true }));
  console.log(JSON.stringify({ candidateRouteTrace: true, mixedAndZeroSends: true, preservedAttemptTotals: true, nestedDuplicatePath: true, managementAuthRequired: true }));
  console.log(JSON.stringify({ conditionRouting: true, projectRequiresCurrentAuthorization: true, opencodeV2ProjectRouting: true, routingProjectPicker: true, explicitModelsAndToolContinuationProtected: true, ruleOrderAndReplayVerified: true }));
  console.log(JSON.stringify({ intentRouting: true, explicitOptIn: true, filteredCurrentUserText: true, staticRulesAndPinnedTurnsProtected: true, timeoutFallback: true, separateClassifierUsage: true, intentReusedAfterPlanWriteFailure: true, unknownIntentNotResent: true, childIntentReusedAfterPlanWriteFailure: true }));
  console.log(JSON.stringify({ passed: true, pluginStopAndRestore: true, pluginBusyResponseProtected: true, stoppedConnectionsUnknown: true, crossProviderCrud: true, staleEditorRejected: true, twoClientCatalogs: true, perModelCapabilities: true, authenticatedCatalogPagination: true, concurrentDefaultPublish: true, specifiedModelPreserved: true, toolTurnPinned: true, oldRequestReplay: true, unknownModelsAndInvalidCapabilitiesRejected: true, configHashesStableAcrossDefaultChange: true, directorySyncAndEncryptedByteRestore: true, serviceObservationRecorded: true, activeServiceDeletionRejected: true, textOnlyOptionalTools: true, forcedUnknownAndHistoricalToolsRejected: true, unexpectedUpstreamToolsBlocked: true, calls: seen.length }));
} finally {
  release?.();
  if (child && child.exitCode === null) { child.kill("SIGTERM"); await once(child, "exit"); }
  for (const server of servers) await new Promise(resolve => server.close(resolve));
  rmSync(directory, { recursive: true, force: true });
}
