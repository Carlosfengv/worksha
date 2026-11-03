import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";

// Own temporary data only. Exercises real service/SQL/grant records; no browser
// hints, real accounts, real Keychain or real model providers are involved.
export async function catalogRecoveryCases({
  api,
  plugin,
  provider,
  scoped,
  control,
  gets,
  posts,
  data,
  report,
}) {
  report.recoveryCases = [];
  const passed = (name) => {
    report.recoveryCases.push(name);
    console.log("通过：" + name);
  };
  const protocol = () => api("/api/provider-operations/protocol");
  const storeId = (await protocol()).storeId;
  const query = { pluginId: plugin.manifest.id, digest: plugin.digest };
  const call = (input, expected = 200) =>
    api("/api/provider-draft-recovery", input, expected);
  const grant = (input, expected = 200) =>
    api("/api/provider-draft-grant", input, expected);
  const status = () => api("/api/provider-catalog/status", query);
  const exists = async (id) =>
    (await status()).bindings.some((b) => b.bindingId === id);
  const files = (owner) => ({
    owner: path.join(
      data,
      "plugins/draft-catalog-owners",
      `${storeId}-${owner.ownerId}.json`,
    ),
    request: path.join(
      data,
      "plugins/draft-catalog-requests",
      `${storeId}-${owner.ownerId}.json`,
    ),
  });
  const open = async (
    scope = {
      catalog: query,
      draft: {
        endpoint: provider.endpoint,
        protocol: provider.protocol,
        vendor: provider.vendor,
        credentialRef: randomUUID(),
      },
      label: "Independent recovery",
      allowPrivateNetwork: true,
    },
  ) => {
    const { owner } = await grant({ kind: "open", storeId, scope });
    const { selection } = await grant({ kind: "acquire", owner });
    return { owner, selection, scope };
  };
  const request = async (selected, kind = "save") => ({
    operation: {
      storeId,
      operationId: randomUUID(),
      expectedSequence: (await protocol()).sequence,
      kind,
      provider: {
        ...provider,
        id: "custom-" + randomUUID(),
        name: "Recovered original provider",
        configured: false,
        revision: 0,
        endpoint: selected.target.endpoint,
        protocol: selected.target.protocol,
        vendor: selected.target.vendor,
        credentialRef: selected.target.credentialRef,
        model: "model-a",
        models: ["model-a"],
      },
    },
    source: { kind: "external", catalog: selected },
    expectedSourceRevision: null,
  });
  const attach = (owner, request) => grant({ kind: "attach", owner, request });
  const inspect = async (owner) =>
    (await call({ kind: "inspect", owner })).inspection;
  const finish = async (owner, released = true) =>
    assert.deepEqual(await call({ kind: "finish", owner }), {
      kind: "finished",
      owner,
      released,
    });
  const cancel = (request) =>
    api("/api/provider-operations/cancel", request.operation);
  const list = async () => (await call({ kind: "list", storeId })).entries;
  const beforeGets = gets(),
    beforePosts = posts();
  const start = await protocol(),
    originalStatus = await status();
  assert.deepEqual(await list(), []);
  await call({ kind: "list", storeId: randomUUID() }, 409);
  await call(
    { kind: "finish", owner: { storeId, ownerId: randomUUID() }, force: true },
    422,
  );
  assert.deepEqual(await status(), originalStatus);
  assert.equal((await protocol()).sequence, start.sequence);
  passed("独立恢复列表无记录可确认；错数据库和强制字段拒绝，零权限与业务写入");

  const a = await open(),
    r = await request(a.selection),
    p = files(a.owner);
  await attach(a.owner, r);
  assert.equal(fs.statSync(p.request).mode & 0o777, 0o600);
  assert.equal(fs.statSync(path.dirname(p.request)).mode & 0o777, 0o700);
  const journal = fs.readFileSync(p.request);
  assert.deepEqual(JSON.parse(journal).request, r);
  const seq = (await protocol()).sequence;
  assert.equal((await list())[0].state, "unchecked");
  const first = await inspect(a.owner);
  assert.equal(first.entry.state, "not-found");
  assert.equal(first.entry.canFinish, false);
  assert.deepEqual(first.request, r.operation);
  await finish(a.owner, false);
  assert(await exists(a.selection.bindingId));
  assert.equal((await protocol()).sequence, seq);
  await cancel(r);
  assert.equal((await inspect(a.owner)).entry.state, "cancelled");
  await finish(a.owner);
  await finish(a.owner);
  assert(!fs.existsSync(p.owner) && !fs.existsSync(p.request));
  assert(!(await exists(a.selection.bindingId)));
  assert.equal(
    (
      await api("/api/provider-source-operations", {
        kind: "commit",
        request: r,
      })
    ).status,
    "cancelled",
  );
  passed(
    "无任何界面提示时逐条恢复未准入原输入，原取消终态后精确收尾，原回执与防重保持",
  );

  const b = await open(),
    rb = await request(b.selection),
    pb = files(b.owner);
  await attach(b.owner, rb);
  const small = JSON.parse(fs.readFileSync(pb.owner));
  small.operation = null;
  fs.writeFileSync(pb.owner, JSON.stringify(small), { mode: 0o600 });
  const smallBytes = fs.readFileSync(pb.owner);
  assert.equal((await list())[0].state, "unchecked");
  assert.deepEqual((await inspect(b.owner)).request, rb.operation);
  assert.deepEqual(fs.readFileSync(pb.owner), smallBytes);
  await finish(b.owner, false);
  await cancel(rb);
  await finish(b.owner);
  passed(
    "请求副本先落盘而小索引未落盘的中断可恢复，核查不改旧格式且不误判为空闲引用",
  );

  const c = await open(),
    rc = await request(c.selection),
    pc = files(c.owner);
  await attach(c.owner, rc);
  const original = JSON.parse(fs.readFileSync(pc.request));
  for (const change of [
    (v) => {
      v.version = 99;
    },
    (v) => {
      v.request.operation.provider.name = "changed";
    },
    (v) => {
      v.request.source.catalog.bindingId = "invalid";
    },
  ]) {
    const v = structuredClone(original);
    change(v);
    const bytes = Buffer.from(JSON.stringify(v));
    fs.writeFileSync(pc.request, bytes);
    assert.equal((await list())[0].state, "unchecked");
    await call({ kind: "inspect", owner: c.owner }, 503);
    await call({ kind: "finish", owner: c.owner }, 503);
    assert.deepEqual(fs.readFileSync(pc.request), bytes);
    assert(await exists(c.selection.bindingId));
  }
  fs.writeFileSync(pc.request, JSON.stringify(original));
  fs.chmodSync(pc.request, 0o644);
  await call({ kind: "inspect", owner: c.owner }, 503);
  assert.equal(fs.statSync(pc.request).mode & 0o777, 0o644);
  fs.chmodSync(pc.request, 0o600);
  await cancel(rc);
  await finish(c.owner);
  passed(
    "未来版本、原输入篡改、错误 nonce 与不安全文件保留并拒绝核查或删除；列表不解码正文",
  );

  const d = await open(),
    rd = await request(d.selection),
    pd = files(d.owner);
  await attach(d.owner, rd);
  fs.unlinkSync(pd.request);
  const old = fs.readFileSync(pd.owner);
  const legacy = await inspect(d.owner);
  assert.equal(legacy.entry.state, "not-found");
  assert.equal(legacy.request, null);
  assert(!legacy.entry.canFinish);
  await finish(d.owner, false);
  assert.deepEqual(fs.readFileSync(pd.owner), old);
  await cancel(rd);
  assert.deepEqual((await inspect(d.owner)).request, rd.operation);
  await finish(d.owner);
  passed(
    "旧版仅哈希归属且 SQL 缺失时不补造取消输入；实际原回执出现后仍可恢复与收尾",
  );

  const e = await open(),
    re = await request(e.selection),
    pe = files(e.owner);
  await attach(e.owner, re);
  fs.unlinkSync(pe.owner);
  let orphan = (await list())[0];
  assert.equal(orphan.ownership, "missing");
  assert.equal(orphan.pluginId, null);
  assert.deepEqual((await inspect(e.owner)).request, re.operation);
  await finish(e.owner, false);
  await cancel(re);
  await finish(e.owner);
  assert(await exists(e.selection.bindingId));
  assert(!fs.existsSync(pe.request));
  await scoped(storeId, {
    ...query,
    action: { kind: "unbind", bindingId: e.selection.bindingId },
  });
  passed(
    "归属元数据丢失时可取消原请求，只清理终态请求副本，绝不猜测并删除旧授权",
  );

  const f = await open(),
    rf = await request(f.selection),
    pf = files(f.owner);
  await attach(f.owner, rf);
  const committed = await api("/api/provider-source-operations", {
    kind: "commit",
    request: rf,
  });
  assert.equal(committed.status, "committed");
  const seqf = (await protocol()).sequence;
  assert.equal((await inspect(f.owner)).entry.canFinish, true);
  await finish(f.owner);
  assert.deepEqual(
    await api("/api/provider-source-operations", {
      kind: "query",
      request: rf,
    }),
    committed,
  );
  assert.equal((await protocol()).sequence, seqf);
  assert(!fs.existsSync(pf.owner) && !fs.existsSync(pf.request));
  passed("另一恢复入口读取已保存原结果并收尾；Provider、原 SQL 回执和序号保持");

  const shared = await open();
  await scoped(storeId, {
    ...query,
    action: {
      kind: "bind",
      target: shared.selection.target,
      allowPrivateNetwork: true,
    },
  });
  const borrowed = await open(shared.scope);
  assert.equal((await inspect(shared.owner)).entry.ownership, "shared");
  await finish(shared.owner);
  await finish(borrowed.owner);
  assert(await exists(shared.selection.bindingId));
  await scoped(storeId, {
    ...query,
    action: { kind: "unbind", bindingId: shared.selection.bindingId },
  });
  assert.equal(gets(), beforeGets);
  assert.equal(posts(), beforePosts);
  passed(
    "显式共享与借用引用独立结束，既有授权不被接管；以上核查零目录和模型发送",
  );

  const live = await open({
      ...shared.scope,
      draft: { ...shared.scope.draft, credentialRef: provider.credentialRef },
    }),
    rl = await request(live.selection, "connect");
  await attach(live.owner, rl);
  control.mode = "hold";
  const arrived = new Promise((resolve) => {
    control.waiting = resolve;
  });
  const pending = api("/api/provider-source-operations", {
    kind: "commit",
    request: rl,
  });
  let timer;
  try {
    await Promise.race([
      arrived,
      pending.then((v) => {
        throw Error("Expected pending: " + v.status);
      }),
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(Error("Synthetic probe not started")),
          12000,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
  assert.equal((await inspect(live.owner)).entry.state, "pending");
  await finish(live.owner, false);
  await cancel(rl);
  await finish(live.owner);
  for (const response of control.held) response.destroy();
  control.waiting = null;
  control.mode = "ok";
  assert.equal((await pending).status, "cancelled");
  assert.equal(
    (
      await api("/api/provider-source-operations", {
        kind: "commit",
        request: rl,
      })
    ).status,
    "cancelled",
  );
  assert.equal(posts(), beforePosts + 1);
  passed(
    "合成请求在途不可收尾；原取消后独立恢复不发送重试，迟到与重放只返回同一终态",
  );
  report.recoverySyntheticPosts = posts() - beforePosts;
  let keeper;
  return {
    async beforeRestart() {
      keeper = await open();
      keeper.request = await request(keeper.selection);
      await attach(keeper.owner, keeper.request);
    },
    async afterRestart() {
      const before = gets() + posts();
      assert.equal((await inspect(keeper.owner)).entry.state, "not-found");
      await cancel(keeper.request);
      await finish(keeper.owner);
      assert(!(await exists(keeper.selection.bindingId)));
      assert.equal(gets() + posts(), before);
      passed(
        "实际服务重启和插件失效后仍可独立读取原请求、取消与收尾，无续权或网络发送",
      );
    },
  };
}
