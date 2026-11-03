import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";
import http from "node:http";
import { DatabaseSync } from "node:sqlite";

const binary = path.resolve(
  process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ??
    "target/debug/local-agent-service",
);
const root = fs.realpathSync(
  fs.mkdtempSync(path.join(tmpdir(), "local-agent-mcp-lifecycle-")),
);
const data = path.join(root, "data");
const home = path.join(root, "source");
fs.mkdirSync(home);
const env = {
  ...process.env,
  LOCAL_AGENT_SOURCE_HOME: home,
  LOCAL_AGENT_TEST_MEMORY_VAULT: "1",
  LOCAL_AGENT_TEST_IDLE_AGENTS: "1",
};
delete env.LOCAL_AGENT_VAULT_WORKER;
const token = randomUUID();
const report = {
  complete: false,
  binarySha256: createHash("sha256")
    .update(fs.readFileSync(binary))
    .digest("hex"),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
  stopDeadlineMs: 1000,
};
const key = path.join(root, "publisher.key");
const timers = new Set();
const controllers = new Set();
let child,
  base,
  metadata,
  mode = "stream",
  stdioPid,
  requests = 0,
  toolCalls = 0;
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function poll(predicate, message, budget = 2500) {
  const until = performance.now() + budget;
  while (performance.now() < until) {
    if (await predicate()) return;
    await pause(10);
  }
  throw Error(message);
}
const upstream = http.createServer(async (req, res) => {
  let body = "";
  for await (const chunk of req) body += chunk;
  const value = body ? JSON.parse(body) : {};
  requests++;
  if (value.method === "tools/call") toolCalls++;
  if (
    req.method === "DELETE" ||
    (req.method === "POST" && value.id === undefined)
  ) {
    res.writeHead(202);
    res.end();
    return;
  }
  if (req.method === "GET" && mode === "hold-headers") return;
  res.writeHead(200, { "content-type": "text/event-stream" });
  if (req.method === "GET" || value.params?.name === "wait") {
    const send = () =>
      res.write(
        'data: {"jsonrpc":"2.0","method":"notifications/progress"}\n\n',
      );
    send();
    const timer = setInterval(send, 5);
    timers.add(timer);
    res.on("close", () => {
      clearInterval(timer);
      timers.delete(timer);
    });
    return;
  }
  res.end(
    `data: ${JSON.stringify({ jsonrpc: "2.0", id: value.id, result: { content: [{ type: "text", text: "fixture-result" }] } })}\n\n`,
  );
});
async function api(
  route,
  value,
  method = value === undefined ? "GET" : "POST",
) {
  const response = await fetch(base + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    ...(value === undefined ? {} : { body: JSON.stringify(value) }),
    signal: AbortSignal.timeout(8000),
  });
  const result = await response.json();
  assert.equal(response.status, 200, `${route}: ${JSON.stringify(result)}`);
  return result;
}
const ref = (plugin) => ({
  pluginId: plugin.manifest.id,
  digest: plugin.digest,
});
const state = (plugin, action) =>
  api("/api/plugins/state", { ...ref(plugin), action });
const action = (plugin, kind) =>
  api("/api/plugin-extensions/action", { ...ref(plugin), action: { kind } });
const status = (plugin) => api("/api/plugin-extensions/status", ref(plugin));
async function install(name) {
  const file = path.join(root, `${name}.laplug`);
  execFileSync(
    binary,
    [
      "--plugin-sdk",
      "pack",
      path.resolve(`sdk/plugins/examples/${name}/package`),
      file,
      key,
    ],
    { env, stdio: "pipe" },
  );
  const inspected = await api("/api/plugins/inspect", { path: file });
  const installed = await api("/api/plugins/install", {
    packageRef: inspected.packageRef,
    digest: inspected.digest,
    approvePublisher: true,
  });
  return state(installed, "enable");
}
async function start(plugin, connection) {
  const file = path.join(root, `${plugin.manifest.id}.json`);
  fs.writeFileSync(file, JSON.stringify(connection));
  await api("/api/plugin-extensions/resource", { ...ref(plugin), path: file });
  await action(plugin, "start-connector");
  return Object.values(
    (await api("/api/plugin-extensions/export", ref(plugin))).mcpServers,
  )[0];
}
async function stream(entry) {
  const controller = new AbortController();
  controllers.add(controller);
  const response = await fetch(entry.url, {
    headers: entry.headers,
    signal: controller.signal,
  });
  assert.equal(response.status, 200);
  const reader = response.body.getReader();
  const view = { chunks: 0, ended: false, controller };
  view.done = (async () => {
    try {
      while (!(await reader.read()).done) view.chunks++;
    } catch {
    } finally {
      view.ended = true;
      controllers.delete(controller);
    }
  })();
  await poll(() => view.chunks >= 3, "fixture did not establish a busy stream");
  return view;
}
async function stopped(view) {
  await poll(
    () => view.ended,
    "MCP stream continued after stop/revocation",
    1000,
  );
  const count = view.chunks;
  await pause(60);
  assert.equal(view.chunks, count);
  await view.done;
}
const passed = (name) => {
  report.cases.push(name);
  console.log(`通过：${name}`);
};
try {
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  execFileSync(binary, ["--plugin-sdk", "keygen", key], { env, stdio: "pipe" });
  child = spawn(binary, ["--data-dir", data], {
    env: { ...env, LOCAL_AGENT_SERVICE_TOKEN: token },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr.resume();
  const lines = createInterface({ input: child.stdout });
  const [line] = await once(lines, "line", {
    signal: AbortSignal.timeout(15000),
  });
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  const plugin = await install("mcp-http");
  const connection = {
    transport: "http",
    url: `http://127.0.0.1:${upstream.address().port}/mcp`,
    allowPrivateNetwork: true,
    providerId: null,
  };
  let entry = await start(plugin, connection);
  const revoked = await stream(entry);
  await state(plugin, "disable");
  await stopped(revoked);
  passed("持续 HTTP SSE 停用后在一秒内停止，已结束连接不再发布");
  await state(plugin, "enable");
  entry = await start(plugin, connection);
  const explicit = await stream(entry);
  await action(plugin, "stop-connector");
  await stopped(explicit);
  passed("再授权创建新入口，显式停止终止持续流");
  entry = await start(plugin, connection);
  mode = "hold-headers";
  const beforeHeaders = requests;
  const waiting = fetch(entry.url, {
    headers: entry.headers,
    signal: AbortSignal.timeout(3000),
  })
    .then((r) => r.status)
    .catch(() => 0);
  await poll(
    () => requests > beforeHeaders,
    "upstream header wait did not start",
  );
  await action(plugin, "stop-connector");
  const failure = await waiting;
  assert([0, 502, 403].includes(failure));
  mode = "stream";
  passed("等待 HTTP 响应头时停止可结束在途请求");
  entry = await start(plugin, connection);
  const rpc = (value) =>
    fetch(entry.url, {
      method: "POST",
      headers: { ...entry.headers, "content-type": "application/json" },
      body: JSON.stringify(value),
      signal: AbortSignal.timeout(6000),
    });
  const waitingTool = await rpc({
    jsonrpc: "2.0",
    id: 1,
    method: "tools/call",
    params: { name: "wait" },
  });
  const reading = waitingTool.text().then(
    () => false,
    () => true,
  );
  await rpc({
    jsonrpc: "2.0",
    method: "notifications/cancelled",
    params: { requestId: 1 },
  });
  assert.equal(await reading, true);
  await poll(
    async () =>
      (await status(plugin)).calls.some((c) => c.status === "cancelled"),
    "cancelled tool metadata did not settle",
  );
  passed("持续工具进度不会推迟按 JSON-RPC ID 取消");
  metadata = new DatabaseSync(path.join(data, "plugin-extensions.sqlite3"));
  metadata.exec(
    "CREATE TRIGGER reject_fixture_call BEFORE INSERT ON connector_calls BEGIN SELECT RAISE(ABORT,'fixture'); END;",
  );
  const callsBefore = toolCalls;
  const rowsBefore = metadata
    .prepare("SELECT count(*) n FROM connector_calls")
    .get().n;
  const success = await rpc({
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: { name: "echo" },
  });
  assert.match(await success.text(), /fixture-result/);
  await poll(async () => {
    const s = await status(plugin);
    return (
      s.connector.observationIncomplete === true &&
      s.connector.unconfirmedCalls === 1
    );
  }, "metadata failure did not become an explicit gap");
  assert.equal(toolCalls, callsBefore + 1);
  assert.equal(
    metadata.prepare("SELECT count(*) n FROM connector_calls").get().n,
    rowsBefore,
  );
  metadata.exec("DROP TRIGGER reject_fixture_call");
  const after = await rpc({
    jsonrpc: "2.0",
    id: 3,
    method: "tools/call",
    params: { name: "echo" },
  });
  assert.match(await after.text(), /fixture-result/);
  await poll(async () => {
    const s = await status(plugin);
    return (
      s.calls.length === rowsBefore + 1 && s.connector.unconfirmedCalls === 1
    );
  }, "confirmed metadata or the earlier gap was lost");
  assert.equal(toolCalls, callsBefore + 2);
  passed(
    "观测 SQLite 写入失败保留成功工具结果、明确缺口且不重放工具；后续记录可继续写入",
  );
  await action(plugin, "stop-connector");
  const stdioPlugin = await install("mcp-stdio");
  const fixture = path.join(root, "flood.py");
  const pidfile = path.join(root, "stdio.pid");
  const receivedfile = path.join(root, "received-tool");
  fs.writeFileSync(
    fixture,
    `import json,os,sys,threading,time\nopen(sys.argv[1],'w').write(str(os.getpid()))\nlock=threading.Lock()\ndef send(v):\n with lock: print(json.dumps(v),flush=True)\ndef flood():\n while True:\n  send({'jsonrpc':'2.0','method':'notifications/progress'})\n  time.sleep(0.001)\nfor line in sys.stdin:\n r=json.loads(line)\n if r.get('method')=='tools/call': open(sys.argv[2],'w').write(str(r['id']))\n if r.get('method')=='initialize': send({'jsonrpc':'2.0','id':r['id'],'result':{'protocolVersion':'2025-11-25','capabilities':{},'serverInfo':{'name':'fixture','version':'1'}}})\n if r.get('method')=='notifications/initialized': threading.Thread(target=flood,daemon=True).start()\n`,
  );
  const stdio = await start(stdioPlugin, {
    transport: "stdio",
    command: "/usr/bin/python3",
    args: [fixture, pidfile, receivedfile],
  });
  const notify = (value) =>
    fetch(stdio.url, {
      method: "POST",
      headers: { ...stdio.headers, "content-type": "application/json" },
      body: JSON.stringify(value),
      signal: AbortSignal.timeout(3000),
    });
  await notify({ jsonrpc: "2.0", method: "notifications/initialized" });
  const notifications = await stream(stdio);
  await poll(() => fs.existsSync(pidfile), "stdio fixture did not start");
  const pid = Number(fs.readFileSync(pidfile, "utf8"));
  stdioPid = pid;
  const pendingReply = notify({
    jsonrpc: "2.0",
    id: 42,
    method: "tools/call",
    params: { name: "wait" },
  }).then(
    (response) => response.status,
    () => 0,
  );
  await poll(
    () => fs.existsSync(receivedfile),
    "stdio fixture did not receive the pending tool",
  );
  await state(stdioPlugin, "disable");
  await stopped(notifications);
  const pendingStatus = await pendingReply;
  assert(pendingStatus === 0 || pendingStatus >= 400);
  await poll(
    async () =>
      (await status(stdioPlugin)).calls.some(
        (call) => call.rpcId === 42 && call.status === "incomplete",
      ),
    "stopped stdio tool did not retain an incomplete record",
  );
  await poll(() => {
    try {
      process.kill(pid, 0);
      return false;
    } catch (error) {
      assert.equal(error.code, "ESRCH");
      return true;
    }
  }, "owned stdio process survived revocation");
  passed("持续 stdio 通知撤权后停止转发、结算未完成调用并回收所属子进程");
  report.complete = true;
} finally {
  for (const controller of controllers) controller.abort();
  metadata?.close();
  if (child?.exitCode === null) {
    try {
      const done = once(child, "exit", { signal: AbortSignal.timeout(12000) });
      await api("/api/runtime/shutdown", {});
      await done;
    } catch {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGKILL");
        await once(child, "exit", { signal: AbortSignal.timeout(5000) });
      }
    }
  }
  for (const timer of timers) clearInterval(timer);
  upstream.closeAllConnections();
  await new Promise((resolve) => upstream.close(resolve));
  report.processExited =
    !!child && (child.exitCode !== null || child.signalCode !== null);
  if (stdioPid) {
    try {
      const command = execFileSync(
        "ps",
        ["-p", String(stdioPid), "-o", "command="],
        { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
      );
      assert(
        command.includes(path.join(root, "flood.py")),
        "refuse to stop an unrelated process",
      );
      process.kill(stdioPid, "SIGKILL");
    } catch (error) {
      if (error.status !== 1 && error.code !== "ESRCH") throw error;
    }
  }
  fs.rmSync(root, { recursive: true, force: true });
  report.temporaryRootRemoved = !fs.existsSync(root);
  const output = process.env.MCP_LIFECYCLE_REPORT;
  if (output) {
    fs.mkdirSync(path.dirname(output), { recursive: true });
    fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
  }
}
console.log(JSON.stringify(report, null, 2));
