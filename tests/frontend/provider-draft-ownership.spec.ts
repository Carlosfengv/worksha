import { test, expect, type Page } from "@playwright/test";
import type {} from "./provider-editor-source-harness";
const ownerPrefix = "local-agent.provider-draft-owner.v1.";
async function open(page: Page, mode = "lost") {
  await page.goto(`/tests/frontend/provider-editor-source.html?mode=${mode}`);
  await page
    .getByRole("button", { name: "选择模型目录来源", exact: true })
    .click();
  await page
    .getByRole("combobox", { name: "模型目录来源", exact: true })
    .click();
  await page.getByRole("option", { name: /外部测试目录/ }).click();
  await page
    .getByRole("checkbox", {
      name: "允许所选目录访问本机或私有网络地址",
      exact: true,
    })
    .check();
  await page.getByRole("checkbox", { name: /同意为本次连接授权/ }).check();
  await page.getByRole("button", { name: "高级设置", exact: true }).click();
}
const snapshot = (page: Page) =>
  page.evaluate(() => ({
    ownerKeys: Object.keys(localStorage).filter((k) =>
      k.startsWith("local-agent.provider-draft-owner.v1."),
    ),
    owners: Object.keys(localStorage)
      .filter((k) => k.startsWith("local-agent.provider-draft-owner.v1."))
      .map((k) => localStorage.getItem(k)),
    pending: window.editorSourceFixture.records(),
    calls: window.editorSourceFixture.ownerCalls,
    bindings: window.editorSourceFixture.currentBindings(),
    unbinds: window.editorSourceFixture.unbinds,
    saves: window.editorSourceFixture.saves.length,
    commands: window.editorSourceFixture.operations?.commands,
  }));

test("closing an unknown editor retains authority, then a different recovery view reaps only its confirmed terminal grant", async ({
  page,
}) => {
  await open(page);
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  const before = await snapshot(page);
  expect(before.calls).toContain("attach");
  expect(before.ownerKeys).toHaveLength(1);
  await page
    .getByRole("button", { name: "关闭并保留记录", exact: true })
    .click();
  expect((await snapshot(page)).unbinds).toHaveLength(0);
  expect((await snapshot(page)).pending.records[0].raw).toBe(
    before.pending.records[0].raw,
  );
  await page.evaluate(() => {
    window.editorSourceFixture.operations!.failQueries = false;
  });
  await page.getByRole("button", { name: "核查原结果", exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).unbinds.length).toBe(1);
  await expect
    .poll(async () => (await snapshot(page)).ownerKeys.length)
    .toBe(0);
  expect((await snapshot(page)).pending.records).toHaveLength(0);
  expect((await snapshot(page)).saves).toBe(1);
});

test("a lost Acquire reply is recoverable on close without guessing from before/after binding lists", async ({
  page,
}) => {
  await open(page, "new");
  await page.evaluate(() => {
    window.editorSourceFixture.loseAcquireReply = true;
  });
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect(page.getByText(/合成授权回复丢失/)).toBeVisible();
  expect((await snapshot(page)).bindings).toHaveLength(1);
  expect(await page.evaluate(() => window.editorSourceFixture.reads)).toBe(0);
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).unbinds.length).toBe(1);
  expect((await snapshot(page)).bindings).toHaveLength(0);
  expect((await snapshot(page)).ownerKeys).toHaveLength(0);
});

test("failure to persist ownership stops before Acquire, directory read or Provider submission", async ({
  page,
}) => {
  await open(page, "new");
  await page.evaluate((prefix) => {
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith(prefix))
        throw new DOMException("受管归属写入失败", "QuotaExceededError");
      return original.call(this, key, value);
    };
  }, ownerPrefix);
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect(page.getByText(/受管归属写入失败/)).toBeVisible();
  const result = await snapshot(page);
  expect(result.calls).toEqual(["open", "release"]);
  expect(result.bindings).toHaveLength(0);
  expect(result.saves).toBe(0);
  expect(result.ownerKeys).toHaveLength(0);
});

test("borrowed preexisting grants survive explicit editor close", async ({
  page,
}) => {
  await open(page, "preexisting-grant");
  const original = (await snapshot(page)).bindings;
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.editorSourceFixture.reads))
    .toBe(1);
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect
    .poll(async () => (await snapshot(page)).ownerKeys.length)
    .toBe(0);
  expect((await snapshot(page)).unbinds).toHaveLength(0);
  expect((await snapshot(page)).bindings).toEqual(original);
});

test("a full browser store during close does not strand a known exclusive idle grant", async ({
  page,
}) => {
  await open(page, "new");
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.editorSourceFixture.reads))
    .toBe(1);
  await page.evaluate((prefix) => {
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith(prefix))
        throw new DOMException("受管关闭写入失败", "QuotaExceededError");
      return original.call(this, key, value);
    };
  }, ownerPrefix);
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect
    .poll(async () => (await snapshot(page)).ownerKeys.length)
    .toBe(0);
  expect((await snapshot(page)).bindings).toHaveLength(0);
  expect((await snapshot(page)).unbinds).toHaveLength(1);
});

test("local ownership correlation failure precedes the original recovery hint and any business submission", async ({
  page,
}) => {
  await open(page, "new");
  await page.evaluate((prefix) => {
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith(prefix) && JSON.parse(value).operationId)
        throw new DOMException("受管关联写入失败", "QuotaExceededError");
      return original.call(this, key, value);
    };
  }, ownerPrefix);
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByText(/受管关联写入失败/)).toBeVisible();
  const result = await snapshot(page);
  expect(result.pending.records).toHaveLength(0);
  expect(result.calls).not.toContain("attach");
  expect(result.saves).toBe(0);
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect
    .poll(async () => (await snapshot(page)).ownerKeys.length)
    .toBe(0);
});

test("an unmount while Attach is in flight preserves the original request and lets recovery retry that same operation once", async ({
  page,
}) => {
  await open(page, "new");
  await page.evaluate(() => {
    window.editorSourceFixture.holdAttach = true;
  });
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect
    .poll(async () => (await snapshot(page)).calls.includes("attach"))
    .toBe(true);
  const before = await snapshot(page);
  await page.evaluate(() => window.editorSourceFixture.closeEditor());
  // Observe React's unmount before releasing the held reply; setting state
  // and resolving in the same task has not unmounted the editor yet.
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.evaluate(() => {
    window.editorSourceFixture.holdAttach = false;
    window.editorSourceFixture.release?.();
  });
  expect((await snapshot(page)).saves).toBe(0);
  expect((await snapshot(page)).unbinds).toHaveLength(0);
  expect((await snapshot(page)).pending.records[0].raw).toBe(
    before.pending.records[0].raw,
  );
  await page.getByRole("button", { name: "重试原请求", exact: true }).click();
  await expect.poll(async () => (await snapshot(page)).saves).toBe(1);
  await expect
    .poll(async () => (await snapshot(page)).ownerKeys.length)
    .toBe(0);
  const result = await snapshot(page);
  const attachments = result.commands?.filter(
    (c) =>
      c.name === "perform_provider_draft_grant" &&
      (c.args as { input: { kind: string } }).input.kind === "attach",
  );
  expect(attachments).toHaveLength(2);
  expect(attachments?.[0].args).toEqual(attachments?.[1].args);
  expect(result.unbinds).toHaveLength(1);
});

test("a mismatched ownership reply cannot authorize reading and its prepared handle still closes safely", async ({
  page,
}) => {
  await open(page, "new");
  await page.evaluate(() => {
    window.editorSourceFixture.badOwnerReply = true;
  });
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect(
    page.getByText(
      /草稿目录授权.*无法|草稿目录授权.*无效|草稿目录授权.*不符合/,
    ),
  ).toBeVisible();
  expect(await page.evaluate(() => window.editorSourceFixture.reads)).toBe(0);
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect
    .poll(async () => (await snapshot(page)).ownerKeys.length)
    .toBe(0);
  expect((await snapshot(page)).unbinds).toHaveLength(1);
});

test("explicit consent after revocation starts a fresh owner instead of replaying the revoked handle", async ({
  page,
}) => {
  await open(page, "new");
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.editorSourceFixture.reads))
    .toBe(1);
  await page.evaluate(() => window.editorSourceFixture.revokeOwnedGrants());
  const consent = page.getByRole("checkbox", { name: /同意为本次连接授权/ });
  await consent.uncheck();
  await consent.check();
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.editorSourceFixture.reads))
    .toBe(2);
  expect(
    await page.evaluate(() => window.editorSourceFixture.binds.length),
  ).toBe(2);
});

test("an explicit admission rejection keeps the pinned original request available for cancellation and grant finalization", async ({
  page,
}) => {
  await open(page, "ownership-rejected");
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  const before = await snapshot(page);
  expect(before.calls).toContain("attach");
  expect(before.pending.records).toHaveLength(1);
  expect(before.saves).toBe(0);
  expect(before.bindings).toHaveLength(1);
  await page
    .getByRole("button", { name: "关闭并保留记录", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect((await snapshot(page)).unbinds).toHaveLength(0);
  await page.getByRole("button", { name: "取消原操作", exact: true }).click();
  await expect
    .poll(async () => (await snapshot(page)).ownerKeys.length)
    .toBe(0);
  const result = await snapshot(page);
  expect(result.unbinds).toHaveLength(1);
  expect(result.pending.records).toHaveLength(0);
  expect(result.saves).toBe(0);
});

test("damaged ownership cannot revoke a guessed binding or block querying the original business result", async ({
  page,
}) => {
  await open(page);
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "关闭并保留记录", exact: true })
    .click();
  await page.evaluate((prefix) => {
    const key = Object.keys(localStorage).find((k) => k.startsWith(prefix))!;
    localStorage.setItem(key, "unreadable-owned-fixture");
    window.editorSourceFixture.operations!.failQueries = false;
  }, ownerPrefix);
  await page.getByRole("button", { name: "核查原结果", exact: true }).click();
  await expect(
    page.getByText("原操作已确认保存。", { exact: true }),
  ).toBeVisible();
  expect((await snapshot(page)).unbinds).toHaveLength(0);
  expect((await snapshot(page)).owners).toEqual(["unreadable-owned-fixture"]);
  expect((await snapshot(page)).pending.records).toHaveLength(0);
});
