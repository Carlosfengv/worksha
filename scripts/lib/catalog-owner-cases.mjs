import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";

// Actual host records, signed guest and existing SQL receipts on owned data.
export async function catalogOwnerCases({
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
  report.ownerCases = [];
  const passed = (name) => {
    report.ownerCases.push(name);
    console.log("通过：" + name);
  };
  const protocol = () => api("/api/provider-operations/protocol");
  const initial = await protocol(),
    storeId = initial.storeId;
  const query = { pluginId: plugin.manifest.id, digest: plugin.digest };
  const status = () => api("/api/provider-catalog/status", query);
  const call = (input, expected = 200) =>
    api("/api/provider-draft-grant", input, expected);
  const owners = [],
    bindings = new Set();
  const scope = () => ({
    catalog: query,
    draft: {
      endpoint: provider.endpoint,
      protocol: provider.protocol,
      vendor: provider.vendor,
      credentialRef: randomUUID(),
    },
    label: "Owned lifecycle",
    allowPrivateNetwork: true,
  });
  const open = async (value = scope()) => {
    const result = await call({ kind: "open", storeId, scope: value });
    assert.equal(result.state, "prepared");
    assert.equal(result.selection, null);
    owners.push(result.owner);
    return result.owner;
  };
  const acquire = async (owner) => {
    const result = await call({ kind: "acquire", owner });
    assert(result.selection);
    bindings.add(result.selection.bindingId);
    return result;
  };
  const release = (owner) => call({ kind: "release", owner });
  const target = (s) => ({ kind: "draft", label: s.label, ...s.draft });
  const legacyBind = (s) =>
    scoped(storeId, {
      ...query,
      action: {
        kind: "bind",
        target: target(s),
        allowPrivateNetwork: s.allowPrivateNetwork,
      },
    });
  const grantExists = async (id) =>
    (await status()).bindings.some((b) => b.bindingId === id);
  const request = async (selection, kind = "save") => {
    const p = await protocol();
    const t = selection.target;
    assert.equal(t.kind, "draft");
    return {
      operation: {
        storeId,
        operationId: randomUUID(),
        expectedSequence: p.sequence,
        kind,
        provider: {
          ...provider,
          id: "custom-" + randomUUID(),
          name: "Owned lifetime provider",
          configured: false,
          revision: 0,
          endpoint: t.endpoint,
          protocol: t.protocol,
          vendor: t.vendor,
          credentialRef: t.credentialRef,
          model: "model-a",
          models: ["model-a"],
          modelCatalog: null,
        },
      },
      source: { kind: "external", catalog: selection },
      expectedSourceRevision: null,
    };
  };
  const attach = (owner, request, expected = 200) =>
    call({ kind: "attach", owner, request }, expected);
  let keeper;
  const beforeGets = gets(),
    beforePosts = posts();
  try {
    const before = await status(),
      first = await open();
    assert.deepEqual(await status(), before);
    assert.equal((await protocol()).sequence, initial.sequence);
    const ownerFile = path.join(
      data,
      "plugins/draft-catalog-owners",
      `${storeId}-${first.ownerId}.json`,
    );
    assert.equal(fs.statSync(ownerFile).mode & 0o777, 0o600);
    await call({ kind: "open", storeId: randomUUID(), scope: scope() }, 409);
    await call({ kind: "release", owner: first, force: true }, 422);
    await call(
      { kind: "acquire", owner: { storeId, ownerId: randomUUID() } },
      404,
    );
    passed(
      "准备记录无授权 / 目录 / Vault 或 SQL 副作用；额外字段、伪造归属与错误数据库拒绝",
    );

    const a = await acquire(first);
    assert.equal(a.state, "acquired");
    assert.equal(
      (await acquire(first)).selection.bindingId,
      a.selection.bindingId,
    );
    await release(first);
    await release(first);
    assert(!(await grantExists(a.selection.bindingId)));
    await call({ kind: "acquire", owner: first }, 404);
    passed(
      "丢失授权回复可用原归属核查；关闭幂等，迟到 Acquire 不能重建已回收授权",
    );

    const legacyScope = scope();
    const legacy = (await legacyBind(legacyScope)).bindings.find(
      (b) =>
        b.label === legacyScope.label &&
        b.endpoint === legacyScope.draft.endpoint &&
        b.authorized &&
        !b.providerId,
    );
    assert(legacy);
    bindings.add(legacy.bindingId);
    const borrowedOwner = await open(legacyScope),
      borrowed = await acquire(borrowedOwner);
    assert.equal(borrowed.state, "borrowed");
    assert.equal(borrowed.selection.bindingId, legacy.bindingId);
    await release(borrowedOwner);
    assert(await grantExists(legacy.bindingId));
    await scoped(storeId, {
      ...query,
      action: { kind: "unbind", bindingId: legacy.bindingId },
    });
    passed("既有授权仅借用，不因编辑器终态回收而删除或接管");

    const sharedScope = scope(),
      x = await open(sharedScope),
      y = await open(sharedScope);
    const sx = await acquire(x),
      sy = await acquire(y);
    assert.equal(sy.state, "acquired");
    assert.equal(sx.selection.bindingId, sy.selection.bindingId);
    await release(x);
    assert(await grantExists(sx.selection.bindingId));
    await release(y);
    assert(!(await grantExists(sx.selection.bindingId)));
    passed("两个受管编辑器只释放自身引用，最后一个归属结束才删除同一授权");

    const adoptedScope = scope(),
      adoptedOwner = await open(adoptedScope),
      adopted = await acquire(adoptedOwner);
    const reused = await legacyBind(adoptedScope);
    assert(
      reused.bindings.some((b) => b.bindingId === adopted.selection.bindingId),
    );
    await release(adoptedOwner);
    assert(await grantExists(adopted.selection.bindingId));
    await scoped(storeId, {
      ...query,
      action: { kind: "unbind", bindingId: adopted.selection.bindingId },
    });
    passed("旧入口明确复用后的授权转为共享，原创建编辑器不能自动删除");

    const pinnedOwner = await open(),
      pinned = await acquire(pinnedOwner),
      r = await request(pinned.selection);
    await attach(pinnedOwner, r);
    await attach(pinnedOwner, r);
    assert.equal((await release(pinnedOwner)).state, "pending");
    assert(await grantExists(pinned.selection.bindingId));
    const other = structuredClone(r);
    other.operation.operationId = randomUUID();
    await attach(pinnedOwner, other, 400);
    await api("/api/provider-operations/cancel", r.operation);
    await release(pinnedOwner);
    assert(!(await grantExists(pinned.selection.bindingId)));
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
      "归属固定到原请求；未准入 / 未知结果保留，原取消终态可独立回收且重放不发送",
    );

    const savedOwner = await open(),
      saved = await acquire(savedOwner),
      save = await request(saved.selection);
    await attach(savedOwner, save);
    const committed = await api("/api/provider-source-operations", {
      kind: "commit",
      request: save,
    });
    assert.equal(committed.status, "committed");
    const sequence = (await protocol()).sequence;
    await release(savedOwner);
    assert(!(await grantExists(saved.selection.bindingId)));
    assert.deepEqual(
      await api("/api/provider-source-operations", {
        kind: "query",
        request: save,
      }),
      committed,
    );
    assert.equal((await protocol()).sequence, sequence);
    assert.equal(gets(), beforeGets);
    assert.equal(posts(), beforePosts);
    passed(
      "另一个恢复视图仅持归属即可在实际提交终态回收；原来源回执仍可查询，零目录与模型发送",
    );

    const liveScope = scope();
    liveScope.draft.credentialRef = provider.credentialRef;
    const liveOwner = await open(liveScope),
      live = await acquire(liveOwner),
      connect = await request(live.selection, "connect");
    await attach(liveOwner, connect);
    control.mode = "hold";
    const arrived = new Promise((resolve) => {
      control.waiting = resolve;
    });
    const pending = api("/api/provider-source-operations", {
      kind: "commit",
      request: connect,
    });
    let timeout;
    try {
      await Promise.race([
        arrived,
        pending.then((value) => {
          throw Error("Expected held synthetic probe: " + value.status);
        }),
        new Promise((_, reject) => {
          timeout = setTimeout(
            () => reject(Error("Owned probe did not arrive")),
            12000,
          );
        }),
      ]);
    } finally {
      clearTimeout(timeout);
    }
    assert.equal((await release(liveOwner)).state, "pending");
    assert(await grantExists(live.selection.bindingId));
    await api("/api/provider-operations/cancel", connect.operation);
    await release(liveOwner);
    for (const response of control.held) response.destroy();
    control.mode = "ok";
    control.waiting = null;
    assert.equal((await pending).status, "cancelled");
    assert.equal(posts(), beforePosts + 1);
    assert.equal(
      (
        await api("/api/provider-source-operations", {
          kind: "commit",
          request: connect,
        })
      ).status,
      "cancelled",
    );
    assert.equal(posts(), beforePosts + 1);
    passed(
      "实际验证请求在途不回收；取消原操作后精确回收，迟到回复与重放均不重复发送",
    );
    report.ownerSyntheticPosts = posts() - beforePosts;
    keeper = true;
    return {
      async beforeRestart() {
        const owner = await open(),
          acquired = await acquire(owner);
        keeper = { owner, bindingId: acquired.selection.bindingId };
      },
      async afterRestart() {
        assert(keeper && typeof keeper === "object");
        const before = gets() + posts();
        const denied = await call(
          { kind: "acquire", owner: keeper.owner },
          400,
        );
        assert(["grant_revoked", "plugin_disabled"].includes(denied.code));
        await release(keeper.owner);
        assert(!(await grantExists(keeper.bindingId)));
        assert.equal(gets() + posts(), before);
        passed("实际服务重启不续期原授权；原归属仍可独立回收，零网络发送");
      },
    };
  } finally {
    if (!keeper) {
      for (const owner of owners) await release(owner).catch(() => null);
      for (const b of (await status()).bindings)
        if (bindings.has(b.bindingId))
          await scoped(storeId, {
            ...query,
            action: { kind: "unbind", bindingId: b.bindingId },
          });
    }
  }
}
