import { test, expect } from "@playwright/test";
import type {} from "./plugin-center-harness";
test("一个列表按安装来源展示，搜索和来源筛选不暴露内部分类", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-center.html");
  const table = page
    .getByLabel("插件中心列表", { exact: true })
    .getByRole("table");
  await expect(table.getByRole("row")).toHaveCount(6);
  await expect(page.getByRole("tablist")).toHaveCount(0);
  await expect(table).toContainText("系统自带");
  await expect(table).toContainText("用户上传");
  await page.getByRole("combobox", { name: "插件来源", exact: true }).click();
  await page.getByRole("option", { name: "用户上传", exact: true }).click();
  await expect(table.getByRole("row")).toHaveCount(4);
  await expect(table).not.toContainText("系统自带");
  await page
    .getByRole("textbox", { name: "搜索插件", exact: true })
    .fill("不存在的插件");
  await expect(table).toContainText("没有匹配的插件");
  expect(await page.evaluate(() => window.centerCalls)).toEqual([]);
});
for (const mode of ["plugin", "source", "policy"]) {
  test(`同一上传入口识别 ${mode}，确认后按顺序安装与启用`, async ({ page }) => {
    await page.goto(`/tests/frontend/plugin-center.html?mode=${mode}`);
    await page.getByRole("button", { name: "上传插件", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("来源：用户上传");
    await expect(
      dialog.getByRole("button", { name: "安装并启用", exact: true }),
    ).toBeDisabled();
    expect(await page.evaluate(() => window.centerCalls)).toEqual(["inspect"]);
    await dialog.getByRole("checkbox").check();
    await dialog
      .getByRole("button", { name: "安装并启用", exact: true })
      .click();
    await expect(dialog).toHaveCount(0);
    const calls = await page.evaluate(() => window.centerCalls);
    expect(calls).toEqual(
      mode === "policy"
        ? ["inspect", "policy:install", "policy:state"]
        : ["inspect", `${mode}:install`, `${mode}:enable`],
    );
    const names: Record<string, string> = {
      plugin: "Aurora 只读会话",
      source: "完整只读历史",
      policy: "反向候选排序",
    };
    const row = page.getByRole("row").filter({ hasText: names[mode] });
    await expect(row).toContainText("运行中");
    await expect(row.getByRole("switch")).toBeChecked();
    if (mode !== "policy") await expect(row).toContainText("数据文件待配置");
  });
}
test("校验拒绝或用户取消不能触发安装和运行", async ({ page }) => {
  for (const mode of ["invalid", "cancel"]) {
    await page.goto(`/tests/frontend/plugin-center.html?mode=${mode}`);
    await page.getByRole("button", { name: "上传插件", exact: true }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    if (mode === "invalid")
      await expect(
        page.getByText("插件能力不受支持", { exact: false }),
      ).toBeVisible();
    expect(await page.evaluate(() => window.centerCalls)).toEqual(["inspect"]);
  }
});
test("干净实例从随附包接入 DSH，仍需确认安装并另选会话文件", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=bundled-dsh");
  const install = page.getByRole("button", {
    name: "安装随附 DSH",
    exact: true,
  });
  await install.click();
  const review = page.getByRole("dialog");
  await expect(review).toContainText("DSH 桌面端 · 1.2.0");
  await expect(review).toContainText("来源：随应用提供");
  await expect(
    review.getByRole("button", { name: "安装并启用" }),
  ).toBeDisabled();
  expect(await page.evaluate(() => window.centerCalls)).toEqual([
    "inspect:bundled-dsh",
  ]);
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "安装并启用" }).click();
  await expect(review).toHaveCount(0);
  await expect(install).toHaveCount(0);
  const dsh = page.getByRole("row").filter({ hasText: "DSH 桌面端" });
  await expect(dsh.getByRole("switch")).toBeChecked();
  await expect(dsh).toContainText("数据文件待配置");
  expect(await page.evaluate(() => window.centerCalls)).toEqual([
    "inspect:bundled-dsh",
    "source:install",
    "source:enable",
  ]);
});
test("DSH 和原生 Agent 共用列表和详情，模型连接入口独立于会话来源", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=agent-parity");
  const table = page
    .getByLabel("Agent 列表", { exact: true })
    .getByRole("table");
  const openCode = table.getByRole("row").filter({ hasText: "OpenCode" });
  await expect(openCode).toHaveCount(1);
  await openCode.getByRole("link", { name: "详情 OpenCode" }).click();
  await expect(page.getByRole("tab", { name: "概览" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByRole("tab", { name: "调用" })).toBeVisible();
  await expect(page.getByRole("tab", { name: "Sessions" })).toBeVisible();
  await page.getByRole("link", { name: "返回 Agents" }).click();
  const dsh = table
    .getByRole("row")
    .filter({ hasText: "DeepSeek Harness 桌面端" });
  await expect(dsh).toContainText("待接入");
  await expect(
    dsh.getByRole("combobox", { name: "DeepSeek Harness 桌面端 模型连接" }),
  ).toBeEnabled();
  await dsh.getByRole("link", { name: "详情 DeepSeek Harness 桌面端" }).click();
  const overview = page.getByRole("tab", { name: "概览" });
  await expect(overview).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab", { name: "调用" })).toBeVisible();
  await expect(page.getByRole("tab", { name: "Sessions" })).toBeVisible();
  const connection = page.getByRole("tab", { name: "连接与观测" });
  await connection.click();
  await expect(page.getByRole("button", { name: "安装随附 DSH" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("link", { name: "管理来源插件" }),
  ).toHaveAttribute("href", "#/plugins?scene=agents");
  await expect(
    page.getByRole("combobox", { name: "DeepSeek Harness 桌面端 模型连接" }),
  ).toBeEnabled();
  expect(await page.evaluate(() => window.centerCalls)).toEqual([]);
});
test("DSH 已启用的会话来源仍可在自身详情授权", async ({ page }) => {
  await page.goto(
    "/tests/frontend/plugin-center.html?mode=agent-parity-source#/agents/net.worksha.dsh-desktop%3Adesktop?tab=connection",
  );
  await expect(page.getByText("来源文件：未授权")).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "DeepSeek Harness 桌面端 模型连接" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "读取会话数据" }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "选择数据文件" }).click();
  await expect(page.getByText("来源文件：fixture.jsonl")).toBeVisible();
  await expect(page.getByLabel("连接方式与采集范围")).toContainText(
    "已授权读取 · 待授权统一展示",
  );
  await expect(
    page.getByRole("button", { name: "读取会话数据" }),
  ).toBeEnabled();
  await expect(page).toHaveURL(
    /#\/agents\/net\.worksha\.dsh-desktop%3Adesktop\?tab=connection$/,
  );
  await page.getByRole("link", { name: "返回 Agents" }).click();
  const dsh = page
    .getByRole("row")
    .filter({ hasText: "DeepSeek Harness 桌面端" });
  await expect(dsh).toHaveCount(1);
  await expect(dsh).toContainText("待授权统一展示");
  expect(await page.evaluate(() => window.centerCalls)).toEqual([
    "source:authorize-file",
  ]);
});
test("DSH 已安装多个停用版本时在插件中心选择要启用的版本", async ({ page }) => {
  await page.goto(
    "/tests/frontend/plugin-center.html?mode=agent-parity-versions",
  );
  await page
    .getByRole("link", { name: "详情 DeepSeek Harness 桌面端" })
    .click();
  await page.getByRole("tab", { name: "连接与观测" }).click();
  await expect(
    page.getByText("选择要启用的版本", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "管理来源插件" }),
  ).toHaveAttribute("href", "#/plugins?scene=agents");
  await expect(page.getByRole("button", { name: "安装随附 DSH" })).toHaveCount(
    0,
  );
  await expect(page.getByRole("button", { name: "启用会话来源" })).toHaveCount(
    0,
  );
  expect(await page.evaluate(() => window.centerCalls)).toEqual([]);
});
test("安装成功但启用失败保留事实，不自动重放，不冒充运行中", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=enable-failed");
  await page.getByRole("button", { name: "上传插件", exact: true }).click();
  await page.getByRole("dialog").getByRole("checkbox").check();
  await page.getByRole("button", { name: "安装并启用", exact: true }).click();
  await expect(
    page.getByText("插件已安装，启用结果待核查。", { exact: false }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.centerCalls)).toEqual([
    "inspect",
    "plugin:install",
    "plugin:enable",
  ]);
  const row = page.getByRole("row").filter({ hasText: "Aurora 只读会话" });
  await expect(row).toContainText("待启用");
  await expect(row.getByRole("switch")).not.toBeChecked();
});
test("部分目录读取失败保留其余插件，不把缺口显示为全部为空", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=partial");
  await expect(
    page.getByText("部分插件目录暂不可用", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByLabel("插件中心列表", { exact: true }).getByRole("table"),
  ).toContainText("模型服务");
  await expect(
    page.getByRole("button", { name: "上传插件", exact: true }),
  ).toBeEnabled();
});

test("核对期间后台变化使原确认失效，不能在新上下文安装", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-center.html");
  await page.getByRole("button", { name: "上传插件", exact: true }).click();
  await page.getByRole("dialog").getByRole("checkbox").check();
  await page.evaluate(() => window.changeCenterBackend());
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText(
    "后台连接或功能状态已变化",
  );
  await expect(
    page.getByRole("button", { name: "安装并启用", exact: true }),
  ).toBeDisabled();
  expect(await page.evaluate(() => window.centerCalls)).toEqual(["inspect"]);
});
test("安装与启用之间后台变化时停止后续写入，保留安装事实", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=context-changed");
  await page.getByRole("button", { name: "上传插件", exact: true }).click();
  await page.getByRole("dialog").getByRole("checkbox").check();
  await page.getByRole("button", { name: "安装并启用", exact: true }).click();
  await expect(
    page.getByText("启用结果待核查", { exact: false }),
  ).toBeVisible();
  expect(await page.evaluate(() => window.centerCalls)).toEqual([
    "inspect",
    "plugin:install",
  ]);
});

test("可用业务插件展示真实入口，待接入功能没有安装和启用入口", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-center.html");
  const models = page
    .getByRole("row")
    .filter({ has: page.locator('a[href="#/model-services"]') });
  await expect(models).toContainText("运行中");
  await expect(models.getByRole("switch")).toBeChecked();
  await models.click();
  const detail = page.getByRole("article", { name: "功能详情", exact: true });
  await expect(detail).toContainText("创建模型服务");
  await expect(
    detail.getByRole("link", { name: "打开模型服务", exact: true }),
  ).toHaveAttribute("href", "#/model-services");
  await expect(page.getByText("任务普通插件待接入")).toBeVisible();
  await expect(page.locator('a[href="#/tasks"]')).toHaveCount(0);
  await expect(page.getByRole("switch", { name: "启用 任务" })).toHaveCount(0);
  const projects = page.getByRole("switch", { name: "启用 项目", exact: true });
  await expect(projects).not.toBeChecked();
  await projects.click();
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(projects).toBeChecked();
  await expect(page.getByText("运行组合", { exact: true })).toHaveCount(0);
});

test("恢复模式可从插件中心取消原操作，恢复功能保留 Providers 停用", async ({
  page,
}) => {
  await page.setViewportSize({ width: 760, height: 720 });
  await page.goto("/tests/frontend/plugin-center.html?mode=recovery");
  const recovery = page.getByRole("region", { name: "服务商操作恢复" });
  await recovery.getByRole("button", { name: "读取后台待核查操作" }).click();
  await expect(
    recovery.getByText("待核查服务商", { exact: false }),
  ).toBeVisible();
  await expect(
    recovery.getByRole("button", { name: "重试原请求" }),
  ).toHaveCount(0);
  await recovery.getByRole("button", { name: "取消原操作" }).click();
  await expect(recovery.getByText("原操作已确认取消。")).toBeVisible();
  await page.getByRole("button", { name: "恢复应用功能" }).click();
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(
    page.getByRole("switch", { name: "启用 Providers", exact: true }),
  ).not.toBeChecked();
  await expect(
    page.getByRole("switch", { name: "启用 Sessions", exact: true }),
  ).toBeChecked();
});

test("从恢复模式单独启用项目时一同恢复其目录支撑", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=recovery");
  const projects = page.getByRole("switch", { name: "启用 项目", exact: true });
  await projects.click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("项目列表 · 启用");
  await dialog.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(projects).toBeChecked();
  await expect(
    page.getByRole("switch", { name: "启用 Providers", exact: true }),
  ).not.toBeChecked();
});
test("按功能场景筛选业务插件与上传扩展，同一数据插件可服务多个场景", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-center.html");
  await page
    .getByRole("group", { name: "功能场景", exact: true })
    .getByRole("button", { name: "项目管理", exact: true })
    .click();
  const table = page
    .getByLabel("插件中心列表", { exact: true })
    .getByRole("table");
  await expect(table).toContainText("完整只读历史");
  await expect(
    table.getByRole("switch", { name: "启用 项目", exact: true }),
  ).not.toBeChecked();
  await page
    .getByRole("group", { name: "功能场景", exact: true })
    .getByRole("button", { name: "任务执行", exact: true })
    .click();
  await expect(
    page.getByText("任务普通插件待接入", { exact: true }),
  ).toBeVisible();
  await expect(table).not.toContainText("完整只读历史");
});

test("系统插件停用前展示连带功能和真实服务，取消不变更", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=dependencies");
  await page
    .getByRole("switch", { name: "启用 Providers", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  for (const name of [
    "Providers",
    "Agents",
    "模型服务",
    "开发服务",
    "团队服务",
  ])
    await expect(dialog).toContainText(name);
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await expect(
    page.getByRole("switch", { name: "启用 Providers", exact: true }),
  ).toBeChecked();
  expect(await page.evaluate(() => window.centerCalls)).toEqual([]);
});
test("依赖停用需明确确认，重新启用消费者只恢复必需依赖", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=dependencies");
  await page
    .getByRole("switch", { name: "启用 Providers", exact: true })
    .click();
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  for (const name of ["Providers", "Agents", "模型服务"])
    await expect(
      page.getByRole("switch", { name: `启用 ${name}`, exact: true }),
    ).not.toBeChecked();
  await page.getByRole("switch", { name: "启用 Agents", exact: true }).click();
  const dialog = page.getByRole("dialog");
  for (const name of ["Providers", "Agents"])
    await expect(dialog).toContainText(name);
  await dialog.getByRole("button", { name: "确认变更", exact: true }).click();
  for (const name of ["Providers", "Agents"])
    await expect(
      page.getByRole("switch", { name: `启用 ${name}`, exact: true }),
    ).toBeChecked();
  await expect(
    page.getByRole("switch", { name: "启用 模型服务", exact: true }),
  ).not.toBeChecked();
  expect(await page.evaluate(() => window.centerCalls)).toEqual([
    "builtin:change",
    "builtin:change",
  ]);
});
test("系统插件可卸载和重新安装，卸载后没有可用入口或假开关", async ({
  page,
}) => {
  await page.goto("/tests/frontend/plugin-center.html");
  const row = page
    .getByRole("row")
    .filter({ hasText: "把不同 Provider 的模型组织" });
  await row.click();
  await page
    .getByRole("article", { name: "功能详情", exact: true })
    .getByRole("button", { name: "卸载", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("保留历史数据与配置");
  await dialog.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(row).toContainText("已卸载");
  await expect(
    page.getByRole("button", { name: "恢复自动更新", exact: true }),
  ).toHaveCount(0);
  await expect(row.getByRole("switch")).toHaveCount(0);
  await expect(row.getByRole("link")).toHaveCount(0);
  await row.getByRole("button", { name: "安装并启用", exact: true }).click();
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(row.getByRole("switch")).toBeChecked();
  await expect(
    row.getByRole("link", { name: "打开功能", exact: true }),
  ).toHaveAttribute("href", "#/model-services");
});
test("运行中任务阻止提交系统插件变更，不自动终止任务", async ({ page }) => {
  await page.goto("/tests/frontend/plugin-center.html?mode=active-task");
  await page
    .getByRole("switch", { name: "启用 Providers", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("alert")).toContainText("请先返回任务页处理");
  await expect(
    dialog.getByRole("button", { name: "确认变更", exact: true }),
  ).toBeDisabled();
  expect(await page.evaluate(() => window.centerCalls)).toEqual([]);
});
