import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  mkdtempSync,
  realpathSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  readdirSync,
  existsSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { createInterface } from "node:readline";

const directory = realpathSync(mkdtempSync(path.join(tmpdir(), "local-agent-project-test-")));
const home = path.join(directory, "home");
const project = path.join(directory, "a", "same");
const other = path.join(directory, "b", "same");
mkdirSync(project, { recursive: true });
mkdirSync(other, { recursive: true });
const git = (...args) => {
  const result = spawnSync("/usr/bin/git", args, {
    cwd: project,
    env: {
      ...process.env,
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_CONFIG_NOSYSTEM: "1",
    },
    encoding: "utf8",
  });
  assert.equal(result.status, 0, "fixture git failed");
};
git("init", "-q");
writeFileSync(path.join(project, "file.txt"), "before\n");
git("add", "file.txt");
git(
  "-c",
  "user.name=Fixture",
  "-c",
  "user.email=fixture@example.invalid",
  "commit",
  "-qm",
  "fixture initial",
);
writeFileSync(path.join(project, "file.txt"), "after-project-body-sentinel\n");
const marker = path.join(directory, "external-hook-must-not-run");
git("config", "core.fsmonitor", `/usr/bin/touch ${marker}`);
git("config", "diff.external", `/usr/bin/touch ${marker}`);
git(
  "config",
  "remote.origin.url",
  "https://user:remote-secret-sentinel@example.invalid/repo.git?token=remote-secret-sentinel",
);
const indexBefore = readFileSync(path.join(project, ".git", "index"));
const today = new Date().toISOString();
const older = new Date(Date.now() - 10 * 86400000).toISOString();
const rows = (agent, id, cwd, time, usage) =>
  agent === "claude"
    ? [
        {
          type: "user",
          sessionId: id,
          uuid: `u-${id}`,
          cwd,
          timestamp: time,
          message: { content: `fixture ${id}` },
        },
        {
          type: "assistant",
          sessionId: id,
          uuid: `a-${id}`,
          timestamp: time,
          message: {
            id: `m-${id}`,
          model: "claude-fixture",
            content: [
              { type: "text", text: "project-content-sentinel" },
              {
                type: "tool_use",
                id: "write-one",
                name: "Write",
                input: {
                  file_path: "new.txt",
                  content: "project-write-body-sentinel",
                },
              },
              { type: "tool_use", id: "mcp-one", name: "mcp__fixture__inspect", input: { value: "fixture" } },
            ],
            ...(usage ? { usage: { input_tokens: 10, output_tokens: 5 } } : {}),
          },
        },
        {
          type: "user",
          sessionId: id,
          uuid: `r-${id}`,
          timestamp: time,
          message: {
            content: [
              {
                type: "tool_result",
                tool_use_id: "write-one",
                is_error: false,
                content: "File created successfully",
              },
              { type: "tool_result", tool_use_id: "mcp-one", content: "fixture returned", is_error: false },
            ],
          },
        },
      ]
    : [
        { type: "session_meta", timestamp: time, payload: { id, cwd } },
        {
          type: "event_msg",
          timestamp: time,
          payload: {
            type: "token_count",
            info: { total_token_usage: { input_tokens: 20, output_tokens: 5 } },
          },
        },
      ];
const writeSource = (agent, id, cwd, time, usage) => {
  const folder = path.join(
    home,
    ...(agent === "claude"
      ? [".claude", "projects", "p"]
      : [".codex", "sessions", "2026", "10", "18"]),
  );
  mkdirSync(folder, { recursive: true });
  writeFileSync(
    path.join(folder, `${id}.jsonl`),
    rows(agent, id, cwd, time, usage)
      .map((row) => JSON.stringify(row))
      .join("\n") + "\n",
  );
};
writeSource("claude", "c", project, today, true);
writeSource("codex", "x", project, older, true);
writeSource("claude", "unknown", other, today, false);
let service;
let listener;
let stderr = "";
const token = randomUUID();
const firstLine = async (child) => {
  const lines = createInterface({ input: child.stdout });
  return Promise.race([
    once(lines, "line").then(([line]) => JSON.parse(line)),
    once(child, "exit").then(() => {
      throw Error("Fixture process exited early");
    }),
    new Promise((_, reject) => {
      const timer = setTimeout(() => reject(Error("Readiness timeout")), 8000);
      timer.unref();
    }),
  ]);
};
const stop = async (child) => {
  if (!child || child.exitCode !== null) return;
  const ended = once(child, "exit");
  child.kill();
  await ended;
};
try {
  listener = spawn(
    process.execPath,
    [
      "-e",
      `const net=require('node:net');const a=net.createServer();a.listen(0,'127.0.0.1',()=>{const port=a.address().port;const b=net.createServer();b.listen({port,host:'::1',ipv6Only:true},()=>console.log(JSON.stringify({port})));});`,
    ],
    {
      cwd: project,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, PRIVATE_ARG_SENTINEL: "process-secret-sentinel" },
    },
  );
  const { port } = await firstLine(listener);
  const startService = () => {
    const child = spawn(
    process.env.LOCAL_AGENT_TEST_SERVICE_BINARY ?? "target/debug/local-agent-service",
    ["--data-dir", path.join(directory, "data")],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SERVICE_TOKEN: token,
        LOCAL_AGENT_SOURCE_HOME: home,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
    child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
    return child;
  };
  service = startService();
  const ready = await firstLine(service);
  let base = `http://127.0.0.1:${ready.port}`;
  const request = async (route, options = {}) =>
    fetch(base + route, {
      ...options,
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
        ...options.headers,
      },
    });
  const json = async (route, allowStale = false) => {
    const result = await request(route);
    // A source scan may change the index while a project snapshot is assembled.
    // Only readiness polling retries this documented conflict; other calls stay strict.
    if (allowStale && result.status === 409) {
      const conflict = await result.clone().json();
      if (conflict.code === "stale_snapshot") return null;
    }
    assert.equal(result.status, 200, await result.clone().text());
    return result.json();
  };
  const wait = async (condition) => {
    for (let attempt = 0; attempt < 65; attempt++) {
      const value = await condition();
      if (value) return value;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    throw Error("Project observation timeout");
  };
  assert.equal((await fetch(base + "/api/projects")).status, 401);
  assert.equal(
    (
      await request("/api/projects", {
        headers: { Origin: "https://example.invalid" },
      })
    ).status,
    401,
  );
  const disabled = await request("/api/projects");
  assert.equal(disabled.status, 409);
  assert.equal((await disabled.json()).code, "plugin_disabled");
  const plugins = await json("/api/builtin-plugins");
  const enabled = await request("/api/builtin-plugins/changes", {
    method: "POST",
    body: JSON.stringify({
      operationId: randomUUID(),
      storeId: plugins.storeId,
      expectedRevision: plugins.revision,
      expectedGeneration: plugins.host.generation,
      composition: { ...plugins.composition, optedInPluginIds: ["builtin.projects"] },
    }),
  });
  assert.equal(enabled.status, 200, await enabled.clone().text());
  for (const agentId of ["claude", "codex"])
    assert.equal(
      (
        await request("/api/session-sources", {
          method: "PUT",
          body: JSON.stringify({ agentId, enabled: true }),
        })
      ).status,
      200,
    );
  await request("/api/sessions/refresh", { method: "POST" });
  const all = await wait(async () => {
    const value = await json("/api/projects", true);
    return value?.total === 2 && value.listeners.asOf ? value : null;
  });
  assert.equal(all.total, 2);
  assert.ok(all.items.every((item) => item.projectKind === "temporary"));
  const regular = await json("/api/projects?scope=workspace");
  assert.equal(regular.total, 0);
  assert.equal(regular.summary.sessionCount, 0);
  assert.equal(regular.summary.usage.totalTokens, 0);
  const temporary = await json("/api/projects?scope=temporary");
  assert.equal(temporary.total, 2);
  assert.equal(temporary.summary.sessionCount, 3);
  assert.equal((await request("/api/projects?scope=invalid")).status, 400);
  assert.equal(all.summary.sessionCount, 3);
  // Codex fixture has only a session cumulative counter, without a model response.
  // Keep that counter in Session context; never allocate it to a client/project response.
  assert.equal(all.summary.usage.totalTokens, 15);
  assert.equal(all.summary.usageScope, "model_responses");
  assert.equal(all.summary.usageUnknownSessions, 1);
  assert.equal(new Set(all.items.map((item) => item.id)).size, 2);
  const item = all.items.find((item) => item.gitRoot);
  assert.equal(item.totals.usage.totalTokens, 15);
  assert.equal(item.agents.length, 2);
  let detail = await json(`/api/projects/${encodeURIComponent(item.id)}`);
  assert.equal(detail.sessions.length, 2);
  assert.equal(
    detail.project.agents.reduce(
      (sum, agent) => sum + (agent.totals.usage.totalTokens ?? 0),
      0,
    ),
    15,
  );
  const unknown = await json(`/api/projects/${encodeURIComponent(item.id)}?agent=unknown`);
  assert.equal(unknown.project.totals.usage.totalTokens, 15);
  assert.ok(unknown.project.agents.every((a) => a.agentId.endsWith("-unknown")));
  const cli = await json(`/api/projects/${encodeURIComponent(item.id)}?agent=codex-cli`);
  assert.equal(cli.sessions.length, 0);
  assert.equal(cli.project.totals.usage.totalTokens, 0);
  const calls = await json(`/api/calls?project=${encodeURIComponent(item.id)}&agent=unknown&kind=model`);
  assert.equal(calls.items.reduce((n, c) => n + (c.usage?.totalTokens ?? 0), 0), 15);
  assert.equal(detail.git.state, "ready");
  assert.equal(detail.git.commits.length, 1);
  assert.ok(!JSON.stringify(detail).includes("remote-secret-sentinel"));
  assert.ok(!JSON.stringify(detail).includes("process-secret-sentinel"));
  const ports = detail.listeners.items.filter((row) => row.port === port);
  assert.equal(ports.length, 2);
  assert.equal(new Set(ports.map((row) => row.id)).size, 2);
  assert.ok(
    ports.every(
      (row) =>
        row.attribution === "project_cwd" &&
        row.processIdentity.includes(row.startedAt),
    ),
  );
  const todayDetail = await json(
    `/api/projects/${encodeURIComponent(item.id)}?days=1`,
  );
  assert.equal(todayDetail.sessions.length, 1);
  assert.equal(todayDetail.project.totals.usage.totalTokens, 15);
  assert.equal(
    todayDetail.listeners.items.filter((row) => row.port === port).length,
    2,
  );
  assert.equal(todayDetail.git.head, detail.git.head);
  assert.equal(detail.writes.length, 1);
  const write = detail.writes[0];
  const conversation = await json(
    `/api/sessions/${encodeURIComponent(write.sessionId)}?anchor=${encodeURIComponent(write.id)}`,
  );
  assert.ok(
    conversation.items.some(
      (event) => event.id === write.id && event.status === "returned",
    ),
  );
  const file = detail.git.files.find((file) => file.path === "file.txt");
  const diff = await json(
    `/api/projects/${encodeURIComponent(item.id)}/diff/${file.id}`,
  );
  assert.ok(diff.text.includes("+after-project-body-sentinel"));
  assert.deepEqual(
    readFileSync(path.join(project, ".git", "index")),
    indexBefore,
  );
  assert.equal(existsSync(marker), false);
  assert.equal(
    (await request(`/api/projects/${encodeURIComponent(item.id)}?days=2`))
      .status,
    400,
  );
  assert.equal((await request("/api/projects/project:missing")).status, 404);
  await stop(listener);
  listener = null;
  detail = await wait(async () => {
    const value = await json(`/api/projects/${encodeURIComponent(item.id)}`, true);
    return value?.listeners.items.every((row) => row.port !== port)
      ? value
      : null;
  });
  assert.equal(
    detail.listeners.items.filter((row) => row.port === port).length,
    0,
  );
  const changeVisibility = (projectId, hidden) => request("/api/project-visibility", {
    method: "PUT", body: JSON.stringify({ projectId, hidden }),
  });
  const sourcesBeforeHide = new Map([
    path.join(home, ".claude/projects/p/c.jsonl"),
    path.join(home, ".claude/projects/p/unknown.jsonl"),
    path.join(home, ".codex/sessions/2026/10/18/x.jsonl"),
    path.join(project, "file.txt"),
  ].map((p) => [p, readFileSync(p)]));
  const nativeCallsBefore = (await json("/api/calls")).total;
  const mcpBefore = await json("/api/mcp");
  const hiddenFacet = mcpBefore.facets.projects.find((p) => p.label === project);
  assert.ok(hiddenFacet);
  assert.equal((await fetch(base + "/api/project-visibility", {method:"PUT",headers:{"Content-Type":"application/json"},body:JSON.stringify({projectId:item.id,hidden:true})})).status,401);
  assert.equal((await changeVisibility("project:invalid",true)).status,400);
  let hide = await changeVisibility(item.id,true);
  assert.equal(hide.status,200);
  const hiddenAt = (await hide.json()).hiddenProjects[0].hiddenAt;
  hide = await changeVisibility(item.id,true);
  assert.equal((await hide.json()).hiddenProjects[0].hiddenAt,hiddenAt);
  for (const scope of ["all","temporary","workspace"]) {
    const list = await json(`/api/projects?scope=${scope}`);
    assert.ok(list.items.every((p) => p.id !== item.id));
    assert.equal(list.summary.usage.totalTokens, scope === "workspace" ? 0 : null);
  }
  assert.equal((await request(`/api/projects/${encodeURIComponent(item.id)}`)).status,409);
  assert.equal((await json("/api/calls")).total,nativeCallsBefore);
  const retainedSessions = await json("/api/sessions");
  assert.equal(retainedSessions.total,3);
  assert.ok(retainedSessions.items.some((s) => s.projectId === item.id && s.projectHidden));
  assert.equal((await json(`/api/sessions/${encodeURIComponent(write.sessionId)}`)).session.projectHidden,true);
  const mcpAfterHide = await json("/api/mcp");
  assert.ok(!mcpAfterHide.facets.projects.some((p) => p.id === hiddenFacet.id));
  assert.equal(mcpAfterHide.totalCalls,mcpBefore.totalCalls);
  assert.equal(mcpAfterHide.usageSummary.relatedTokens,mcpBefore.usageSummary.relatedTokens);
  assert.equal((await request(`/api/mcp?project=${hiddenFacet.id}`)).status,409);
  await stop(service);
  service = startService();
  base = `http://127.0.0.1:${(await firstLine(service)).port}`;
  assert.equal((await json("/api/snapshot")).hiddenProjects[0].id,item.id);
  await wait(async () => (await json("/api/projects",true))?.total === 1);
  for (const agentId of ["claude", "codex"])
    await request("/api/session-sources", {
      method: "PUT",
      body: JSON.stringify({ agentId, enabled: false }),
    });
  assert.equal((await json("/api/projects")).total, 0);
  assert.equal(
    (await request(`/api/projects/${encodeURIComponent(item.id)}`)).status,
    409,
  );
  // Restore remains available after source access is withdrawn.
  assert.equal((await changeVisibility(item.id,false)).status,200);
  assert.equal((await json("/api/snapshot")).hiddenProjects.length,0);
  assert.equal((await request(`/api/projects/${encodeURIComponent(item.id)}`)).status,404);
  for (const agentId of ["claude", "codex"])
    await request("/api/session-sources", {method:"PUT",body:JSON.stringify({agentId,enabled:true})});
  await request("/api/sessions/refresh",{method:"POST"});
  await wait(async () => (await json("/api/projects",true))?.total === 2);
  assert.ok((await json("/api/mcp")).facets.projects.some((p) => p.id === hiddenFacet.id));
  for (const [file, bytes] of sourcesBeforeHide) assert.deepEqual(readFileSync(file),bytes);
  await stop(service);
  service = null;
  for (const file of readdirSync(path.join(directory, "data"))) {
    if (!file.includes("sqlite")) continue;
    const bytes = readFileSync(path.join(directory, "data", file));
    for (const secret of [
      "project-content-sentinel",
      "project-write-body-sentinel",
      "after-project-body-sentinel",
      "remote-secret-sentinel",
      "process-secret-sentinel",
    ])
      assert.equal(bytes.includes(Buffer.from(secret)), false);
  }
  assert.equal(stderr, "");
  console.log(
    "通过：真实临时 Git / 双地址 TCP 监听、跨 Agent 项目与 Token 对账、同名目录分离、日历与当前快照隔离、写入证据下钻、只读差异与配置执行阻断、进程退出更新、来源断开拒绝及正文 / 凭据不落盘。",
  );
} finally {
  await stop(listener);
  await stop(service);
  rmSync(directory, { recursive: true, force: true });
}
