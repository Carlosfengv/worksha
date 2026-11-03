import { expect, test } from "@playwright/test";

test("Agent table exposes a single shared local-entry recovery panel instead of repeating it in client rows", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-switch.html?list=1&local-pending=1",
  );
  await expect(
    page.getByRole("button", { name: "保留原编号并查询", exact: true }),
  ).toHaveCount(1);
  await expect(
    page.getByRole("region", { name: "本机入口结果核查", exact: true }),
  ).toHaveCount(1);
});

const cardName = "配置事务 目标供应商 · new-model";

test("dynamic switching works with running processes and preserves a dismissed preview", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-switch.html?agent=claude-code-cli&dynamic=1",
  );
  await expect(page.getByText("已连接固定动态入口")).toBeVisible();
  await expect(page.getByText(/^当前路由：/)).toHaveCount(0);
  await page
    .getByRole("button", { name: "切换 Provider Claude Code CLI", exact: true })
    .click();
  await page.getByRole("combobox", { name: "目标连接", exact: true }).click();
  await page.getByRole("option", { name: /目标供应商/ }).click();
  await expect(page.getByRole("dialog")).not.toContainText("应用前需关闭相关");
  await page.getByRole("button", { name: "生成动态路由预览" }).click();
  await expect(page.getByRole("dialog")).toContainText(
    "固定入口和 Agent 配置不变",
  );
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "继续确认动态切换" }).click();
  await page.getByRole("button", { name: "确认发布路由" }).click();
  await expect(
    page.getByText("默认目标：目标供应商 · new-model"),
  ).toBeVisible();
  await expect(page.getByText(/^当前路由：/)).toHaveCount(0);
  expect(
    await page.evaluate(() => window.providerSwitchFixture.actions),
  ).toEqual(["dynamic-prepare:new-model", "dynamic-commit"]);
});

test("dynamic conflicts remain visible inside the confirmation and cancel leaves the old target", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-switch.html?agent=claude-code-cli&dynamic=1",
  );
  await page
    .getByRole("button", { name: "切换 Provider Claude Code CLI", exact: true })
    .click();
  await page.getByRole("combobox", { name: "目标连接", exact: true }).click();
  await page.getByRole("option", { name: /目标供应商/ }).click();
  await page.getByRole("button", { name: "生成动态路由预览" }).click();
  await page.evaluate(() => {
    window.providerSwitchFixture.failNext = true;
  });
  await page.getByRole("button", { name: "确认发布路由" }).click();
  await expect(page.getByRole("dialog")).toContainText("目标版本已变化");
  await page.getByRole("button", { name: "撤销动态预览" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByText("默认目标：原供应商 · old-model")).toBeVisible();
});

test("Agent list selects a model service without changing the current connection on cancel", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-switch.html?list=1");
  const table = page.getByRole("table");
  await expect(table.getByRole("row")).toHaveCount(13);
  await expect(page.getByText("在 Agent 内选择模型")).toBeVisible();
  const selector = page.getByRole("combobox", {
    name: "Claude Code CLI 模型服务",
    exact: true,
  });
  await expect(selector).toBeEnabled();
  await selector.click();
  await expect(
    page.getByRole("option", { name: "目标供应商", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("option", { name: "编程服务", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("接入模型服务");
  await expect(dialog).toContainText("Claude Code CLI → 编程服务");
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(selector).toContainText("选择模型服务");
  expect(
    await page.evaluate(() => window.providerSwitchFixture.actions),
  ).toEqual([]);
  expect(new URL(page.url()).hash).toBe("");
});

test("Agent list keeps shared Codex configuration pending until confirmed", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-switch.html?list=1&ready=1");
  const desktop = page.getByRole("combobox", {
    name: "Codex 桌面端 Provider",
    exact: true,
  });
  await expect(desktop).toBeEnabled();
  await desktop.click();
  await page.getByRole("option", { name: "目标供应商", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Codex CLI 和 Codex 桌面端");
  await dialog.getByRole("button", { name: "生成切换预览与备份" }).click();
  await expect(dialog).toContainText("重启 Agent 并生效");
  await dialog
    .getByRole("button", { name: "稍后手动重启", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  for (const name of ["Codex CLI", "Codex 桌面端"]) {
    const row = page
      .getByRole("row")
      .filter({ has: page.getByRole("heading", { name, exact: true }) });
    await expect(row).toContainText("待重启后生效");
    await expect(row.getByRole("combobox")).toContainText("原供应商");
  }
  expect(
    await page.evaluate(() => window.providerSwitchFixture.actions),
  ).toEqual(["prepare:codex:target-provider:new-model", "defer"]);
});

test("Agent list supports keyboard selection and recovers from configuration read failure", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/provider-switch.html?list=1");
  const selector = page.getByRole("combobox", {
    name: "OpenCode 模型服务",
    exact: true,
  });
  await expect(selector).toBeEnabled();
  await selector.focus();
  await selector.press("ArrowDown");
  await page
    .getByRole("option", { name: "编程服务", exact: true })
    .press("Enter");
  await expect(page.getByRole("dialog")).toContainText("接入模型服务");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "取消", exact: true })
    .click();
  await page.evaluate(() => {
    window.providerSwitchFixture.readFailure = true;
  });
  await page.getByRole("button", { name: "重新读取配置", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("隔离配置读取失败");
  await expect(selector).toBeDisabled();
  await page.evaluate(() => {
    window.providerSwitchFixture.readFailure = false;
  });
  await page.getByRole("button", { name: "重新读取配置", exact: true }).click();
  await expect(selector).toBeEnabled();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("pending service notices wrap inside Agent table cells", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/provider-switch.html?list=1&pending=1");
  const notice = page.getByRole("status").filter({ hasText: "有未完成的切换" });
  await expect(notice).toBeVisible();
  await expect(
    notice.getByRole("button", { name: "撤销待应用切换", exact: true }),
  ).toBeVisible();
  expect(
    await notice.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
});

for (const agent of [
  "opencode",
  "claude-code-desktop",
  "codex-cli",
  "codex-desktop",
]) {
  test(`${agent} group requires every published model to verify before restart`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/provider-switch.html?ready=1&expired=1&group=1&agent=${agent}`,
    );
    const card = page.getByRole("article", { name: cardName });
    await expect(card).toContainText("待发布 2 个模型");
    await card.getByRole("button", { name: "重启 Agent 并生效" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText(
      "同一 Provider 分组包含 2 个模型：new-model、alternate-model",
    );
    await page.evaluate(() => {
      window.providerSwitchFixture.failModel = "alternate-model";
    });
    await dialog.getByRole("button", { name: "重新验证目标模型" }).click();
    await expect(dialog).toContainText("目标模型验证未完成");
    await expect(
      dialog.getByRole("button", {
        name: agent === "codex-cli" ? "已退出，应用配置" : "确认重启并生效",
      }),
    ).toBeDisabled();
    expect(
      await page.evaluate(() => window.providerSwitchFixture.actions),
    ).toEqual(["verify:new-model", "verify:alternate-model"]);
    await page.evaluate(() => {
      window.providerSwitchFixture.failModel = null;
    });
    await dialog.getByRole("button", { name: "重新验证目标模型" }).click();
    await expect(dialog).toContainText("已验证 2 个模型");
    await expect(
      dialog.getByRole("button", {
        name: agent === "codex-cli" ? "已退出，应用配置" : "确认重启并生效",
      }),
    ).toBeEnabled();
    expect(
      await page.evaluate(() => window.providerSwitchFixture.actions),
    ).toEqual([
      "verify:new-model",
      "verify:alternate-model",
      "verify:new-model",
      "verify:alternate-model",
    ]);
  });
}
test("deferred expired verification can be renewed in place before restart without writing", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-switch.html?ready=1&expired=1");
  const card = page.getByRole("article", { name: cardName });
  await expect(card).toContainText("目标模型需要重新验证");
  await card.getByRole("button", { name: "重启 Agent 并生效" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("可能产生少量费用");
  await expect(
    dialog.getByRole("button", { name: "确认重启并生效" }),
  ).toBeDisabled();
  await page.evaluate(() => {
    window.providerSwitchFixture.failNext = true;
  });
  await dialog.getByRole("button", { name: "重新验证目标模型" }).click();
  await expect(dialog.getByRole("alert")).toContainText(
    "连接暂不可用，配置未修改",
  );
  await expect(
    dialog.getByRole("button", { name: "确认重启并生效" }),
  ).toBeDisabled();
  await dialog.getByRole("button", { name: "重新验证目标模型" }).click();
  await expect(dialog).toContainText("目标模型验证通过");
  await expect(
    dialog.getByRole("button", { name: "确认重启并生效" }),
  ).toBeEnabled();
  expect(
    await page.evaluate(() => window.providerSwitchFixture.actions),
  ).toEqual(["verify", "verify"]);
  await page.keyboard.press("Escape");
  await expect(page.getByText("old-model", { exact: true })).toBeVisible();
});
test("OpenCode can apply after manual exit without requesting a desktop restart; dialog stays bounded", async ({
  page,
}) => {
  await page.setViewportSize({ width: 800, height: 600 });
  await page.goto("/tests/frontend/provider-switch.html?ready=1");
  await page
    .getByRole("article", { name: cardName })
    .getByRole("button", { name: "重启 Agent 并生效" })
    .click();
  const dialog = page.getByRole("dialog");
  expect(
    await dialog.evaluate((node) => node.scrollWidth <= node.clientWidth + 1),
  ).toBe(true);
  await dialog.getByRole("button", { name: "已退出，应用后手动启动" }).click();
  await expect(dialog.getByText("正在应用配置", { exact: true })).toBeVisible();
  await page.evaluate(() => window.providerSwitchFixture.finish?.());
  await expect(dialog).toHaveCount(0);
  expect(
    await page.evaluate(() => window.providerSwitchFixture.actions),
  ).toEqual(["apply"]);
  await expect(page.getByText("配置已写入 · 待重启后生效")).toBeVisible();
});
test("desktop restart has confirmation, progress and launch status; config is read back", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-switch.html?ready=1");
  const card = page.getByRole("article", { name: cardName });
  await card.getByRole("button", { name: "重启 Agent 并生效" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("不会强制结束进程");
  expect(
    await page.evaluate(() => window.providerSwitchFixture.actions),
  ).toEqual([]);
  await dialog.getByRole("button", { name: "确认重启并生效" }).click();
  await expect(
    dialog.getByText("正在重启并应用配置", { exact: true }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "确认重启并生效" }),
  ).toBeDisabled();
  await page.evaluate(() => window.providerSwitchFixture.finish?.());
  await expect(dialog).toHaveCount(0);
  await expect(card).toContainText("配置已写入并已请求启动");
  await expect(page.getByText("new-model", { exact: true })).toBeVisible();
  await expect(page.getByText("已请求启动 · 待调用确认").first()).toBeVisible();
  expect(
    await page.evaluate(() => window.providerSwitchFixture.actions),
  ).toEqual(["restart"]);
});

test("extra CLI process failure remains in the dialog and beside the transaction", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-switch.html");
  const card = page.getByRole("article", { name: cardName });
  await card.getByRole("button", { name: "重启 Agent 并生效" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "确认重启并生效" }).click();
  await expect(dialog.getByRole("alert")).toContainText(
    "配置未修改，桌面端未退出",
  );
  await expect(
    dialog.getByRole("button", { name: "确认重启并生效" }),
  ).toBeEnabled();
  await dialog.getByRole("button", { name: "稍后手动重启" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(card).toContainText("已保留待重启切换");
  await expect(page.getByText("old-model", { exact: true })).toBeVisible();
  expect(
    await page.evaluate(() => window.providerSwitchFixture.actions),
  ).toEqual(["restart", "defer"]);
});

for (const agent of [
  "claude-code-cli",
  "claude-code-desktop",
  "codex-cli",
  "codex-desktop",
  "opencode",
]) {
  test(`${agent} can defer without writing; pending status is shown`, async ({
    page,
  }) => {
    await page.goto(`/tests/frontend/provider-switch.html?agent=${agent}`);
    const card = page.getByRole("article", { name: cardName });
    await expect(
      page.getByText("待重启后生效", { exact: true }).first(),
    ).toBeVisible();
    await card.getByRole("button", { name: "稍后手动重启" }).click();
    await expect(card).toContainText("已保留待重启切换");
    await expect(card).toContainText("当前配置未修改");
    await expect(page.getByText("old-model", { exact: true })).toBeVisible();
    expect(
      await page.evaluate(() => window.providerSwitchFixture.actions),
    ).toEqual(["defer"]);
  });
}

test("CLI requires manual exit, supports unknown state, and applies only after recheck", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-switch.html?agent=codex-cli");
  const card = page.getByRole("article", { name: cardName });
  await card.getByRole("button", { name: "重启 Agent 并生效" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("同时影响 Codex CLI 与桌面端");
  await expect(dialog).toContainText("原项目目录");
  await expect(
    dialog.getByRole("button", { name: "已退出，应用配置" }),
  ).toBeDisabled();
  await page.evaluate(() => {
    window.providerSwitchFixture.processes = null;
  });
  await dialog.getByRole("button", { name: "重新检查运行状态" }).click();
  await expect(dialog).toContainText("相关进程尚未退出或状态未知");
  await expect(
    dialog.getByRole("button", { name: "已退出，应用配置" }),
  ).toBeDisabled();
  await page.evaluate(() => {
    window.providerSwitchFixture.processes = 0;
    window.providerSwitchFixture.failNext = true;
  });
  await dialog.getByRole("button", { name: "重新检查运行状态" }).click();
  await expect(
    dialog.getByRole("button", { name: "已退出，应用配置" }),
  ).toBeEnabled();
  await dialog.getByRole("button", { name: "已退出，应用配置" }).click();
  await expect(dialog.getByRole("alert")).toContainText("配置尚未写入");
  await dialog.getByRole("button", { name: "已退出，应用配置" }).click();
  await expect(dialog.getByText("正在应用配置", { exact: true })).toBeVisible();
  await page.evaluate(() => window.providerSwitchFixture.finish?.());
  await expect(dialog).toHaveCount(0);
  await expect(card).toContainText("已写入配置");
  await expect(page.getByText("配置已写入 · 待重启后生效")).toBeVisible();
});
