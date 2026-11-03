import { test, expect, type Page } from "@playwright/test";
import type {} from "./provider-editor-source-harness";
const prefix = "local-agent.provider-draft-owner.v1.";
async function open(page: Page, mode = "ownership-rejected") {
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
async function unknown(page: Page, mode?: string) {
  await open(page, mode);
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.editorSourceFixture.ownerCalls.includes("attach"),
      ),
    )
    .toBe(true);
  await expect(page.getByText("保存结果待核查", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "关闭并保留记录", exact: true })
    .click();
}
async function host(page: Page) {
  await page
    .getByRole("button", { name: "草稿目录授权恢复", exact: true })
    .click();
  await page.getByRole("button", { name: "读取宿主归属", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "核查此归属", exact: true }),
  ).toBeVisible();
}
async function inspect(page: Page) {
  await page.getByRole("button", { name: "核查此归属", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "重新核查此归属", exact: true }),
  ).toBeVisible();
}
const state = (page: Page) =>
  page.evaluate(() => ({
    pending: window.editorSourceFixture.records(),
    commands: window.editorSourceFixture.operations!.commands,
    recoveries: window.editorSourceFixture.recoveryCalls,
    saves: window.editorSourceFixture.saves.length,
    bindings: window.editorSourceFixture.currentBindings(),
    unbinds: window.editorSourceFixture.unbinds,
    hints: Object.keys(localStorage)
      .filter((k) => k.startsWith("local-agent.provider-draft-owner.v1."))
      .map((k) => localStorage.getItem(k)),
  }));
async function finish(page: Page) {
  await page.getByRole("button", { name: "结束此归属", exact: true }).click();
  await page
    .getByRole("button", { name: "确认结束此归属", exact: true })
    .click();
  await expect(
    page.getByText("当前数据库没有可列出的草稿归属或请求副本。", {
      exact: true,
    }),
  ).toBeVisible();
}

test("missing browser hints recover the exact unadmitted request and cancel through the original IPC before finishing", async ({
  page,
}) => {
  await unknown(page);
  const original = (await state(page)).pending.records[0];
  await page.evaluate((prefix) => {
    for (const k of Object.keys(localStorage))
      if (k.startsWith(prefix)) localStorage.removeItem(k);
  }, prefix);
  await host(page);
  expect((await state(page)).recoveries).toEqual(["list"]);
  await inspect(page);
  await expect(
    page.getByRole("button", { name: "结束此归属", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "取消此原操作", exact: true }).click();
  await expect(
    page.getByText("原操作已确认取消；可核对后结束授权归属。", { exact: true }),
  ).toBeVisible();
  await finish(page);
  const after = await state(page);
  const cancel = after.commands.filter(
    (c) => c.name === "cancel_provider_operation",
  );
  expect(cancel).toHaveLength(1);
  expect((cancel[0].args as { request: unknown }).request).toEqual(
    original.request,
  );
  expect(after.pending.records[0].raw).toBe(original.raw);
  expect(after.saves).toBe(0);
  expect(after.bindings).toHaveLength(0);
});

test("corrupt browser authority hints do not block host inspection of a committed result", async ({
  page,
}) => {
  await unknown(page, "lost");
  const before = await state(page);
  await page.evaluate((prefix) => {
    for (const k of Object.keys(localStorage))
      if (k.startsWith(prefix)) localStorage.setItem(k, "{damaged");
  }, prefix);
  await host(page);
  await inspect(page);
  await expect(
    page.getByRole("button", { name: "取消此原操作", exact: true }),
  ).toHaveCount(0);
  await finish(page);
  const after = await state(page);
  expect(after.saves).toBe(1);
  expect(after.pending.records[0].raw).toBe(before.pending.records[0].raw);
  expect(after.hints).toHaveLength(0);
  expect(
    after.commands.filter((c) => c.name === "cancel_provider_operation"),
  ).toHaveLength(0);
});

test("a legacy hash-only missing SQL record stays unknown without inventing cancellation input", async ({
  page,
}) => {
  await unknown(page);
  await page.evaluate(() => window.editorSourceFixture.dropRequestCopies());
  await host(page);
  await inspect(page);
  await expect(page.getByText(/旧记录未保留可恢复的原请求内容/)).toBeVisible();
  await expect(
    page.getByRole("button", { name: "取消此原操作", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "结束此归属", exact: true }),
  ).toBeDisabled();
  expect((await state(page)).bindings).toHaveLength(1);
});

test("host finishing a borrowed idle reference preserves the explicit preexisting permission", async ({
  page,
}) => {
  await open(page, "preexisting-grant");
  const before = (await state(page)).bindings;
  await page.getByRole("button", { name: "获取模型", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.editorSourceFixture.reads))
    .toBe(1);
  await page.evaluate((prefix) => {
    for (const k of Object.keys(localStorage))
      if (k.startsWith(prefix)) localStorage.removeItem(k);
    window.editorSourceFixture.closeEditor();
  }, prefix);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await host(page);
  await inspect(page);
  await finish(page);
  expect((await state(page)).bindings).toEqual(before);
  expect((await state(page)).unbinds).toHaveLength(0);
});

test("a wrong-store inspection reply cannot enable a cancellation or finish", async ({
  page,
}) => {
  await unknown(page);
  await host(page);
  await page.evaluate(() => {
    window.editorSourceFixture.badRecoveryReply = true;
  });
  await page.getByRole("button", { name: "核查此归属", exact: true }).click();
  await expect(
    page.getByText("恢复结果暂不可核查", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "取消此原操作", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "结束此归属", exact: true }),
  ).toHaveCount(0);
  expect((await state(page)).bindings).toHaveLength(1);
});

for (const boundary of ["close", "reconnect"] as const) {
  test(`a held host list cannot publish across ${boundary}`, async ({
    page,
  }) => {
    await unknown(page);
    await page.evaluate(() => {
      window.editorSourceFixture.holdRecovery = "list";
    });
    await page
      .getByRole("button", { name: "草稿目录授权恢复", exact: true })
      .click();
    await page
      .getByRole("button", { name: "读取宿主归属", exact: true })
      .click();
    await expect
      .poll(() =>
        page.evaluate(() => window.editorSourceFixture.recoveryCalls.length),
      )
      .toBe(1);
    if (boundary === "close")
      await page
        .getByRole("button", { name: "关闭恢复核查", exact: true })
        .click();
    else await page.evaluate(() => window.editorSourceFixture.recover());
    await page.evaluate(() => window.editorSourceFixture.release?.());
    if (boundary === "close")
      await page
        .getByRole("button", { name: "草稿目录授权恢复", exact: true })
        .click();
    await expect(
      page.getByText("尚未读取宿主记录。", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "核查此归属", exact: true }),
    ).toHaveCount(0);
    expect((await state(page)).bindings).toHaveLength(1);
  });
}

test("a changed local hint survives a delayed successful host finish", async ({
  page,
}) => {
  await unknown(page, "lost");
  await host(page);
  await inspect(page);
  await page.evaluate(() => {
    window.editorSourceFixture.holdRecovery = "finish";
  });
  await page.getByRole("button", { name: "结束此归属", exact: true }).click();
  await page
    .getByRole("button", { name: "确认结束此归属", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.editorSourceFixture.recoveryCalls.includes("finish"),
      ),
    )
    .toBe(true);
  await page.evaluate((prefix) => {
    for (const k of Object.keys(localStorage))
      if (k.startsWith(prefix)) localStorage.setItem(k, "{changed-snapshot");
    window.editorSourceFixture.release?.();
  }, prefix);
  await expect(
    page.getByText("当前数据库没有可列出的草稿归属或请求副本。", {
      exact: true,
    }),
  ).toBeVisible();
  expect((await state(page)).hints).toEqual(["{changed-snapshot"]);
  expect((await state(page)).saves).toBe(1);
});

test("explicit local hint removal is compare-and-remove and never mutates host permissions", async ({
  page,
}) => {
  await unknown(page);
  await host(page);
  await page
    .getByRole("button", { name: "人工核对后移除授权提示", exact: true })
    .click();
  await page.evaluate((prefix) => {
    for (const k of Object.keys(localStorage))
      if (k.startsWith(prefix)) localStorage.setItem(k, "{newer-hint");
  }, prefix);
  await page
    .getByRole("button", { name: "已核对，仅移除此提示", exact: true })
    .click();
  await expect(
    page.getByText("恢复记录已变化，未移除原记录。", { exact: true }),
  ).toBeVisible();
  const after = await state(page);
  expect(after.hints).toEqual(["{newer-hint"]);
  expect(after.bindings).toHaveLength(1);
  expect(after.recoveries).toEqual(["list"]);
});

test("an unavailable browser hint store does not block host finishing", async ({
  page,
}) => {
  await unknown(page, "lost");
  const original = (await state(page)).pending.records[0].raw;
  await page.evaluate((prefix) => {
    const get = Storage.prototype.getItem;
    Storage.prototype.getItem = function (key) {
      if (key.startsWith(prefix))
        throw new DOMException("受管提示读取失败", "SecurityError");
      return get.call(this, key);
    };
  }, prefix);
  await host(page);
  await expect(
    page.getByText("本机提示暂不可读取", { exact: true }),
  ).toBeVisible();
  await inspect(page);
  await finish(page);
  await expect(
    page.getByText(/宿主归属已结束；本机提示暂未移除/),
  ).toBeVisible();
  const after = await page.evaluate(() => ({
    bindings: window.editorSourceFixture.currentBindings(),
    pending: window.editorSourceFixture.records().records[0].raw,
    saves: window.editorSourceFixture.saves.length,
  }));
  expect(after.bindings).toHaveLength(0);
  expect(after.pending).toBe(original);
  expect(after.saves).toBe(1);
});

test("minimum viewport keeps host recovery reachable by keyboard and scroll", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await unknown(page);
  await host(page);
  const read = page.getByRole("button", { name: "读取宿主归属", exact: true });
  await read.focus();
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "检查本机授权提示", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "核查此归属", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Enter");
  const cancel = page.getByRole("button", {
    name: "取消此原操作",
    exact: true,
  });
  await cancel.focus();
  await expect(cancel).toBeInViewport();
  const dialog = page.getByRole("dialog", {
    name: "草稿目录授权恢复",
    exact: true,
  });
  expect(
    await dialog.evaluate((node) => node.scrollHeight > node.clientHeight),
  ).toBe(true);
  const box = await dialog.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(720);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "草稿目录授权恢复", exact: true }),
  ).toBeFocused();
  expect((await state(page)).bindings).toHaveLength(1);
});

test("inspecting the last page preserves its rows and finishing clamps a smaller inventory", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-editor-source.html?mode=new");
  await page.evaluate(() => {
    window.editorSourceFixture.seedRecoveryOwners(11);
    window.editorSourceFixture.closeEditor();
  });
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page
    .getByRole("button", { name: "草稿目录授权恢复", exact: true })
    .click();
  await page.getByRole("button", { name: "读取宿主归属", exact: true }).click();
  const region = page.getByRole("region", {
    name: "宿主草稿授权归属",
    exact: true,
  });
  await region
    .getByRole("button", { name: "Go to next page", exact: true })
    .click();
  await expect(region.getByText("Page 2 of 2", { exact: true })).toBeVisible();
  const label = await region.locator("tbody tr").innerText();
  await region.getByRole("button", { name: "核查此归属", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "重新核查此归属", exact: true }),
  ).toBeVisible();
  await expect(region.getByText("Page 2 of 2", { exact: true })).toBeVisible();
  expect(await region.locator("tbody tr").innerText()).toContain(
    label.split("\n")[0],
  );
  await page.getByRole("button", { name: "结束此归属", exact: true }).click();
  await page
    .getByRole("button", { name: "确认结束此归属", exact: true })
    .click();
  await expect(region.getByText("Page 1 of 1", { exact: true })).toBeVisible();
  await expect(
    region.getByRole("button", { name: "核查此归属", exact: true }),
  ).toHaveCount(10);
});
