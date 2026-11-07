import { test, expect } from "@playwright/test";

test("旧后台缺少容量信息时不暗示已支持预览复用或放弃", async ({ page }) => {
  await page.goto("/tests/frontend/advanced-plugin.html?mode=config");
  await expect(
    page.getByText(
      "当前后台未提供配置记录容量，尚未确认是否支持预览复用或放弃；请核对后台版本。",
    ),
  ).toBeVisible();
  await expect(page.getByText(/相同的有效预览会复用/)).toHaveCount(0);
  await expect(page.getByText(/配置记录名额：/)).toHaveCount(0);
});

test("配置文件授权、预览与确认执行独立，凭据只展示占位", async ({ page }) => {
  await page.goto("/tests/frontend/advanced-plugin.html?mode=config");
  await expect(
    page.getByRole("button", { name: "生成配置预览" }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "选择并授权配置文件" }).click();
  await page.getByRole("combobox", { name: "扩展使用的连接" }).click();
  await page.getByRole("option").first().click();
  await page.getByRole("button", { name: "生成配置预览" }).click();
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("${HOST_CREDENTIAL}");
  await expect(page.locator("#calls")).toHaveText(
    '["authorize","prepare-config"]',
  );
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  await page
    .getByRole("button", { name: "确认执行配置计划", exact: true })
    .click();
  await expect(
    page.getByRole("cell", { name: "已应用", exact: true }),
  ).toBeVisible();
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  await page
    .getByRole("button", { name: "确认恢复配置备份", exact: true })
    .click();
  await expect(
    page.getByRole("cell", { name: "已恢复", exact: true }),
  ).toBeVisible();
});
test("协议绑定必须选择具体连接并确认，取消不授权", async ({ page }) => {
  await page.goto("/tests/frontend/advanced-plugin.html?mode=protocol");
  await expect(
    page.getByRole("button", { name: "授权协议绑定", exact: true }),
  ).toBeDisabled();
  await page.getByRole("combobox", { name: "扩展使用的连接" }).click();
  await page.getByRole("option").first().click();
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: "授权协议绑定", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("完整模型请求与响应");
  await page.keyboard.press("Escape");
  await expect(page.locator("#calls")).toHaveText("[]");
  await page.getByRole("button", { name: "授权协议绑定", exact: true }).click();
  await page
    .getByRole("button", { name: "确认授权协议绑定", exact: true })
    .click();
  await expect(page.locator("#calls")).toHaveText('["bind-protocol"]');
});
test("MCP 授权不启动、启动不伪造握手或调用，窄窗口能操作", async ({ page }) => {
  await page.setViewportSize({ width: 720, height: 650 });
  await page.goto("/tests/frontend/advanced-plugin.html?mode=mcp");
  await expect(
    page.getByRole("button", { name: "启动 MCP 观测入口", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "选择并授权 MCP 连接文件" }).click();
  await expect(page.locator("#calls")).toHaveText('["authorize"]');
  await page
    .getByRole("button", { name: "启动 MCP 观测入口", exact: true })
    .click();
  await page
    .getByRole("button", { name: "确认启动 MCP 观测入口", exact: true })
    .click();
  await expect(page.getByText(/MCP 连接：尚未确认/)).toBeVisible();
  await expect(page.getByText(/尚未记录工具执行/)).toBeVisible();
  await page.getByRole("button", { name: "导出 MCP 连接" }).click();
  await expect(page.locator("#calls")).toHaveText(
    '["authorize","start-connector","export"]',
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
test("插件移除后仍可恢复备份并看到更新后的状态", async ({ page }) => {
  await page.goto("/tests/frontend/advanced-plugin.html?mode=history");
  await page.getByRole("button", { name: "配置备份与恢复" }).click();
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "确认执行配置计划" }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "确认恢复配置备份", exact: true })
    .click();
  await expect(page.locator("#calls")).toHaveText('["restore-config"]');
  await expect(
    page.getByRole("row").filter({ hasText: "离线连接" }),
  ).toContainText("已恢复");
});

test("放弃未执行预览须明确操作，完成后重新读取名额，已应用记录不提供放弃", async ({
  page,
}) => {
  await page.goto("/tests/frontend/advanced-plugin.html?mode=config");
  await page.getByRole("button", { name: "选择并授权配置文件" }).click();
  await page.getByRole("combobox", { name: "扩展使用的连接" }).click();
  await page.getByRole("option").first().click();
  await page.getByRole("button", { name: "生成配置预览" }).click();
  await expect(page.getByText(/配置记录名额：1 \/ 64，剩余 63/)).toBeVisible();
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "确认放弃未执行预览" }),
  ).toBeEnabled();
  await page.keyboard.press("Escape");
  await expect(page.locator("#calls")).toHaveText(
    '["authorize","prepare-config"]',
  );
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  await page.getByRole("button", { name: "确认放弃未执行预览" }).click();
  await expect(page.getByText(/配置记录名额：0 \/ 64，剩余 64/)).toBeVisible();
  await expect(
    page.getByRole("cell", { name: "离线连接", exact: true }),
  ).toHaveCount(0);
  await expect(page.locator("#calls")).toHaveText(
    '["authorize","prepare-config","discard-config"]',
  );
  await page.goto("/tests/frontend/advanced-plugin.html?mode=history");
  await page.getByRole("button", { name: "配置备份与恢复" }).click();
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "确认放弃未执行预览" }),
  ).toHaveCount(0);
});

test("独立宿主入口可放弃旧预览，失败仍保留记录且不显示释放成功", async ({
  page,
}) => {
  for (const failed of [true, false]) {
    await page.goto(
      `/tests/frontend/advanced-plugin.html?mode=history&prepared=1${failed ? "&discardError=1" : ""}`,
    );
    await page.getByRole("button", { name: "配置备份与恢复" }).click();
    await page.getByRole("cell", { name: "离线连接", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "确认执行配置计划" }),
    ).toHaveCount(0);
    await page.getByRole("button", { name: "确认放弃未执行预览" }).click();
    await expect(page.locator("#calls")).toHaveText('["discard-config"]');
    if (failed) {
      await expect(page.getByRole("dialog")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByText("配置计划操作未完成")).toBeVisible();
      await expect(
        page.getByText("预览状态已变化，请重新读取。"),
      ).toBeVisible();
      await expect(
        page.getByText(/配置记录名额：1 \/ 64，剩余 63/),
      ).toBeVisible();
    } else {
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(
        page.getByText(/配置记录名额：0 \/ 64，剩余 64/),
      ).toBeVisible();
      await expect(page.getByText("尚未生成配置计划")).toBeVisible();
    }
  }
});

test("损坏备份目录显示读取失败，显式重读前不伪造空态或执行恢复", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto(
    "/tests/frontend/advanced-plugin.html?mode=history&historyError=first",
  );
  await page.getByRole("button", { name: "配置备份与恢复" }).click();
  await expect(page.getByText("备份目录读取未完成")).toBeVisible();
  await expect(page.getByText(/记录损坏或版本不兼容/)).toBeVisible();
  await expect(page.getByText("尚未生成配置计划")).toHaveCount(0);
  await page.clock.runFor(30000);
  await expect(page.locator("#history-reads")).toHaveText("1");
  await expect(page.locator("#calls")).toHaveText("[]");
  await page.getByRole("button", { name: "配置备份与恢复" }).click();
  await expect(
    page.getByRole("cell", { name: "离线连接", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("备份目录读取未完成")).toHaveCount(0);
  await expect(page.locator("#history-reads")).toHaveText("2");
  await expect(page.locator("#calls")).toHaveText("[]");
});

test("旧备份目录重新读取失败时保留列表，并暂停已有弹层的恢复操作", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/advanced-plugin.html?mode=history&historyError=second",
  );
  await page.getByRole("button", { name: "配置备份与恢复" }).click();
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  const restore = page.getByRole("button", {
    name: "确认恢复配置备份",
    exact: true,
  });
  await expect(restore).toBeEnabled();
  await page.evaluate(() => window.advancedPluginFixture.recover());
  await expect(page.locator("#history-reads")).toHaveText("2");
  await expect(restore).toBeDisabled();
  await expect(page.locator("#calls")).toHaveText("[]");
  await page.keyboard.press("Escape");
  await expect(page.getByText("备份目录读取未完成")).toBeVisible();
  await expect(
    page.getByRole("cell", { name: "离线连接", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "配置备份与恢复" }).click();
  await page.getByRole("cell", { name: "离线连接", exact: true }).click();
  await expect(restore).toBeEnabled();
  await expect(page.locator("#history-reads")).toHaveText("3");
  await expect(page.locator("#calls")).toHaveText("[]");
});

test("MCP 外部执行状态自动刷新，轮询不主动执行工具", async ({ page }) => {
  await page.clock.install();
  await page.goto(
    "/tests/frontend/advanced-plugin.html?mode=mcp&observation=1",
  );
  await page.getByRole("button", { name: "选择并授权 MCP 连接文件" }).click();
  await page
    .getByRole("button", { name: "启动 MCP 观测入口", exact: true })
    .click();
  await page
    .getByRole("button", { name: "确认启动 MCP 观测入口", exact: true })
    .click();
  await expect(page.getByText(/MCP 连接：尚未确认/)).toBeVisible();
  await page.clock.fastForward(10001);
  await expect(page.getByText(/MCP 连接：握手成功/)).toBeVisible();
  await expect(
    page.getByRole("cell", { name: "echo", exact: true }),
  ).toBeVisible();
  await expect(page.locator("#calls")).toHaveText(
    '["authorize","start-connector"]',
  );
});

test("观测待确认和旧后台覆盖未知不显示确定无调用", async ({ page }) => {
  for (const marker of ["unconfirmed", "legacy"]) {
    await page.goto(
      `/tests/frontend/advanced-plugin.html?mode=mcp&${marker}=1`,
    );
    await page.getByRole("button", { name: "选择并授权 MCP 连接文件" }).click();
    await page
      .getByRole("button", { name: "启动 MCP 观测入口", exact: true })
      .click();
    await page
      .getByRole("button", { name: "确认启动 MCP 观测入口", exact: true })
      .click();
    await expect(page.getByText("观测记录完整性尚未确认")).toBeVisible();
    await expect(page.getByText("暂无已确认的工具观测记录。")).toBeVisible();
    await expect(page.getByText("尚未记录工具执行")).toHaveCount(0);
    await expect(
      page.getByText(
        marker === "unconfirmed"
          ? /有 2 次调用的观测记录尚未确认/
          : /当前后台未提供观测完整性信息/,
      ),
    ).toBeVisible();
    await expect(page.locator("#calls")).toHaveText(
      '["authorize","start-connector"]',
    );
  }
});

test("实际 IPC 解析器兼容旧观测状态并拒绝不可信完整性字段", async ({
  page,
}) => {
  await page.goto("/tests/frontend/advanced-plugin.html?mode=mcp");
  const values = await page.evaluate(async () => {
    const validate = window.advancedPluginFixture.validateConnector;
    const old = await validate({});
    const current = await validate({
      observationIncomplete: false,
      unconfirmedCalls: 0,
    });
    const errors = [];
    for (const value of [
      { observationIncomplete: "yes" },
      { observationIncomplete: true, unconfirmedCalls: -1 },
      { observationIncomplete: false, unconfirmedCalls: 2 },
      { observationIncomplete: true, unconfirmedCalls: 0 },
    ]) {
      try {
        await validate(value);
        errors.push("accepted");
      } catch (error) {
        errors.push(error instanceof Error ? error.message : String(error));
      }
    }
    return {
      old: old.connector?.observationIncomplete,
      current: current.connector?.observationIncomplete,
      errors,
    };
  });
  expect(values.old).toBeUndefined();
  expect(values.current).toBe(false);
  expect(values.errors).toEqual(Array(4).fill("MCP 观测完整性状态无效。"));
});
