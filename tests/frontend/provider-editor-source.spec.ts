import { expect, test, type Page } from "@playwright/test";
import type {} from "./provider-editor-source-harness";
async function external(page: Page, mode = "new") {
  await page.goto(`/tests/frontend/provider-editor-source.html?mode=${mode}`);
  await page.getByRole("dialog").waitFor({ state: "visible" });
  if (
    await page
      .getByRole("button", { name: "选择模型目录来源", exact: true })
      .count()
  )
    await page
      .getByRole("button", { name: "选择模型目录来源", exact: true })
      .click();
  await page
    .getByRole("combobox", { name: "模型目录来源", exact: true })
    .click();
  await page.getByRole("option", { name: /外部测试目录/ }).click();
  await expect(
    page.getByRole("checkbox", { name: /同意为本次连接授权/ }),
  ).toBeEnabled();
  await page
    .getByRole("checkbox", {
      name: "允许所选目录访问本机或私有网络地址",
      exact: true,
    })
    .check();
  await page.getByRole("checkbox", { name: /同意为本次连接授权/ }).check();
}
const fixture = (page: Page) =>
  page.evaluate(() => ({
    binds: window.editorSourceFixture.binds,
    reads: window.editorSourceFixture.reads,
    nativeReads: window.editorSourceFixture.nativeReads,
    unbinds: window.editorSourceFixture.unbinds,
    saves: window.editorSourceFixture.saves,
    commands: window.editorSourceFixture.operations?.commands,
    records: window.editorSourceFixture.records(),
  }));
test("actual editor reads external directory only after consent and saves source atomically through the new command", async ({
  page,
}) => {
  await external(page);
  await page.getByRole("button", { name: "高级设置", exact: true }).click();
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect.poll(async () => (await fixture(page)).reads).toBe(1);
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await fixture(page);
  expect(result.nativeReads).toBe(0);
  expect(result.saves).toHaveLength(1);
  expect(result.saves[0].modelCatalog?.sourceUrl).toMatch(/\/catalog$/);
  expect(
    result.commands?.some(
      (c) => c.name === "perform_provider_source_operation",
    ),
  ).toBe(true);
  expect(
    result.commands?.some((c) => c.name === "commit_provider_operation"),
  ).toBe(false);
  expect(result.binds.every((b) => b.kind === "draft")).toBe(true);
  expect(result.unbinds.length).toBeGreaterThan(0);
});
test("saved external source can be explicitly reset to builtin in the same metadata operation", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-editor-source.html?mode=saved");
  await page
    .getByRole("combobox", { name: "模型目录来源", exact: true })
    .click();
  await page.getByRole("option", { name: "内置模型目录", exact: true }).click();
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await fixture(page);
  const call = result.commands?.find(
    (c) => c.name === "perform_provider_source_operation",
  );
  expect(call?.args).toMatchObject({
    input: {
      kind: "commit",
      request: {
        source: { kind: "builtin" },
        expectedSourceRevision: "b".repeat(64),
      },
    },
  });
  expect(result.binds).toHaveLength(0);
  expect(result.nativeReads).toBe(0);
});
test("unreadable saved source cannot be cleared by choosing builtin", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-editor-source.html?mode=saved-unreadable",
  );
  await expect(
    page.getByText("原来源文件尚不可核查", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "保存连接", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("combobox", { name: "模型目录来源", exact: true })
    .click();
  await page.getByRole("option", { name: "内置模型目录", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "保存连接", exact: true }),
  ).toBeDisabled();
  expect((await fixture(page)).saves).toHaveLength(0);
});
test("a lost source operation preserves version3 recovery and its grant on close", async ({
  page,
}) => {
  await external(page, "lost");
  await page.getByRole("button", { name: "高级设置", exact: true }).click();
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  const before = await fixture(page);
  expect(before.records.records).toHaveLength(1);
  expect(JSON.parse(before.records.records[0].raw).version).toBe(3);
  await page
    .getByRole("button", { name: "关闭并保留记录", exact: true })
    .click();
  const after = await fixture(page);
  expect(after.records.records[0].raw).toBe(before.records.records[0].raw);
  expect(after.unbinds).toHaveLength(0);
});
test("backend recovery while a draft grant is pending retires the late result without submission", async ({
  page,
}) => {
  await external(page, "bind-hold");
  await page.getByRole("button", { name: "高级设置", exact: true }).click();
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect.poll(async () => (await fixture(page)).binds.length).toBe(1);
  await page.evaluate(() => {
    window.editorSourceFixture.recover();
    window.editorSourceFixture.release?.();
  });
  await expect.poll(async () => (await fixture(page)).unbinds.length).toBe(1);
  expect((await fixture(page)).reads).toBe(0);
  expect((await fixture(page)).saves).toHaveLength(0);
});

test("actual automatic connection captures the external directory in its original connect request", async ({
  page,
}) => {
  await external(page);
  await page
    .getByRole("button", { name: "自动识别并验证", exact: true })
    .click();
  await page.getByRole("button", { name: "连接并保存", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await fixture(page);
  expect(
    result.commands?.find((c) => c.name === "perform_provider_source_operation")
      ?.args,
  ).toMatchObject({
    input: {
      kind: "commit",
      request: { operation: { kind: "connect" }, source: { kind: "external" } },
    },
  });
  expect(result.nativeReads).toBe(0);
  expect(result.saves).toHaveLength(1);
});
test("saved directory authorization is an explicit revision-bound action and never edits provider metadata", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-editor-source.html?mode=saved");
  await page.getByRole("checkbox", { name: /同意为本次连接授权/ }).check();
  await page
    .getByRole("button", { name: "授权当前已保存连接读取目录", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(() => window.editorSourceFixture.sourceSelections.length),
    )
    .toBe(1);
  const result = await page.evaluate(() => ({
    selections: window.editorSourceFixture.sourceSelections,
    binds: window.editorSourceFixture.binds,
    saves: window.editorSourceFixture.saves,
  }));
  expect(result.binds).toEqual([
    {
      kind: "saved",
      providerId: "custom-5d160843-43f9-46b9-8486-a3e652cedb49",
      expectedRevision: "1",
    },
  ]);
  expect(result.selections[0][1]).toBe("b".repeat(64));
  expect(result.selections[0][2]?.target.kind).toBe("saved");
  expect(result.saves).toHaveLength(0);
  await expect(
    page.getByRole("checkbox", { name: /同意为本次连接授权/ }),
  ).not.toBeChecked();
});

test("a changed saved scope can refresh its pinned policy before explicit reauthorization and save", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-editor-source.html?mode=saved-changed",
  );
  await page
    .getByRole("button", { name: "核对所选目录策略", exact: true })
    .click();
  await page.getByRole("checkbox", { name: /同意为本次连接授权/ }).check();
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const result = await fixture(page);
  expect(
    result.commands?.find((c) => c.name === "perform_provider_source_operation")
      ?.args,
  ).toMatchObject({
    input: {
      request: {
        expectedSourceRevision: "b".repeat(64),
        source: { kind: "external" },
      },
    },
  });
  expect(result.nativeReads).toBe(0);
});

test("720 by 560 editor keeps source selection, consent and submit reachable with keyboard and scrolling", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/provider-editor-source.html?mode=narrow-keyboard",
  );
  await page
    .getByRole("button", { name: "选择模型目录来源", exact: true })
    .click();
  const combo = page.getByRole("combobox", {
    name: "模型目录来源",
    exact: true,
  });
  await combo.focus();
  await combo.fill("外部测试");
  await combo.press("ArrowDown");
  await combo.press("Enter");
  const privateAccess = page.getByRole("checkbox", {
    name: "允许所选目录访问本机或私有网络地址",
    exact: true,
  });
  await privateAccess.focus();
  await privateAccess.press("Space");
  const consent = page.getByRole("checkbox", { name: /同意为本次连接授权/ });
  await consent.focus();
  await consent.press("Space");
  await expect(consent).toBeChecked();
  const save = page.getByRole("button", { name: "保存连接", exact: true });
  await save.scrollIntoViewIfNeeded();
  await save.focus();
  await save.press("Enter");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect((await fixture(page)).saves).toHaveLength(1);
});
