import { test, expect } from "@playwright/test";
import type {} from "./builtin-plugins-harness";
const prefix = "local-agent.builtin-composition.v1.";
async function review(page: import("@playwright/test").Page) {
  await page
    .getByRole("switch", { name: "启用 Codex 本机来源", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toContainText("Codex 本机来源 · 停用");
}
test("开关显示实际状态，依赖影响待确认，轮询不丢焦点或发起保存", async ({
  page,
}) => {
  await page.goto("/tests/frontend/builtin-plugins.html");
  const source = page.getByRole("switch", {
    name: "启用 项目来源读取",
    exact: true,
    includeHidden: true,
  });
  const assets = page.getByRole("switch", {
    name: "启用 项目资料读取",
    exact: true,
    includeHidden: true,
  });
  await expect(source).toBeChecked();
  await source.click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("项目来源读取 · 停用");
  await expect(dialog).toContainText("项目资料读取 · 停用");
  await expect(source).toBeChecked();
  await expect(assets).toBeChecked();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(source).toBeChecked();
  await expect(source).toBeFocused();
  await source.click();
  await dialog.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(source).not.toBeChecked();
  await expect(assets).not.toBeChecked();
  await expect(assets).toBeEnabled();
  await assets.click();
  await expect(dialog).toContainText("项目来源读取 · 启用");
  await expect(dialog).toContainText("项目资料读取 · 启用");
  await expect(assets).not.toBeChecked();
  await page.keyboard.press("Escape");
  await assets.focus();
  await page.evaluate(() => window.bumpBuiltin(false));
  await expect(assets).toBeFocused();
  await assets.click();
  await page.evaluate(() => window.bumpBuiltin());
  await expect(
    dialog.getByRole("alert").filter({ hasText: "功能状态已变化" }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "确认变更", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(() =>
      window.builtinCalls.filter(
        (c) => c.command === "configure_builtin_plugins",
      ),
    ),
  ).toHaveLength(1);
  expect(await page.evaluate(() => window.builtinDraftCheck())).toBe(true);
});
test("明确确认才提交一次，成功后使用实际运行状态", async ({ page }) => {
  await page.goto("/tests/frontend/builtin-plugins.html");
  await review(page);
  expect(
    await page.evaluate(
      () =>
        window.builtinCalls.filter(
          (c) => c.command === "configure_builtin_plugins",
        ).length,
    ),
  ).toBe(0);
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "插件变更已保存" }),
  ).toBeVisible();
  await expect(
    page.getByRole("switch", { name: "启用 Codex 本机来源", exact: true }),
  ).not.toBeChecked();
  expect(
    await page.evaluate(
      () =>
        window.builtinCalls.filter(
          (c) => c.command === "configure_builtin_plugins",
        ).length,
    ),
  ).toBe(1);
  expect(
    await page.evaluate(
      (p) => Object.keys(localStorage).filter((key) => key.startsWith(p)),
      prefix,
    ),
  ).toEqual([]);
});
test("丢失响应的原请求跨重开保留，查询回执不重放保存", async ({ page }) => {
  await page.goto("/tests/frontend/builtin-plugins.html?mode=unknown");
  await review(page);
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "待核查的插件操作" }),
  ).toBeVisible();
  const before = await page.evaluate(
    (p) =>
      Object.fromEntries(
        Object.entries(localStorage).filter(([key]) => key.startsWith(p)),
      ),
    prefix,
  );
  await page.reload();
  await expect(
    page.getByRole("button", { name: "查询原结果", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(
      () =>
        window.builtinCalls.filter(
          (c) => c.command === "configure_builtin_plugins",
        ).length,
    ),
  ).toBe(0);
  expect(
    await page.evaluate(
      (p) =>
        Object.fromEntries(
          Object.entries(localStorage).filter(([key]) => key.startsWith(p)),
        ),
      prefix,
    ),
  ).toEqual(before);
  await page.getByRole("button", { name: "查询原结果", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "待核查的插件操作" }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        window.builtinCalls.filter(
          (c) => c.command === "configure_builtin_plugins",
        ).length,
    ),
  ).toBe(0);
});
test("未查到的结果保留，重试必须使用原编号和原参数", async ({ page }) => {
  await page.goto("/tests/frontend/builtin-plugins.html?mode=unknown-before");
  await review(page);
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await page.getByRole("button", { name: "查询原结果", exact: true }).click();
  await expect(
    page.getByText("尚未查到已提交回执", { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "按原编号重试", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.builtinCalls.filter(
            (c) => c.command === "configure_builtin_plugins",
          ).length,
      ),
    )
    .toBe(2);
  const calls = await page.evaluate(() =>
    window.builtinCalls.filter(
      (c) => c.command === "configure_builtin_plugins",
    ),
  );
  expect(calls[0].input).toEqual(calls[1].input);
  await expect(
    page.getByRole("switch", { name: "启用 Codex 本机来源", exact: true }),
  ).toBeDisabled();
});
test("其他后台或错误回执不能删除原操作，明确拒绝可重新编辑", async ({
  page,
}) => {
  await page.goto("/tests/frontend/builtin-plugins.html?mode=unknown");
  await review(page);
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "查询原结果", exact: true }),
  ).toBeEnabled();
  await page.evaluate(() => window.corruptBuiltinReceipt());
  await page.getByRole("button", { name: "查询原结果", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "待核查的插件操作" }),
  ).toBeVisible();
  await expect(
    page.getByText("插件状态响应身份或版本不一致", { exact: false }),
  ).toBeVisible();
  const preserved = await page.evaluate(
    (p) => Object.entries(localStorage).filter(([key]) => key.startsWith(p)),
    prefix,
  );
  expect(preserved).toHaveLength(1);
  const foreign = JSON.parse(preserved[0][1]);
  foreign.storeId = "30000000-0000-4000-8000-000000000001";
  await page.evaluate(({ key, value }) => localStorage.setItem(key, value), {
    key: preserved[0][0],
    value: JSON.stringify(foreign),
  });
  await page.reload();
  await expect(
    page.getByRole("button", { name: "查询原结果", exact: true }),
  ).toBeDisabled();
  await page.evaluate(() => {
    localStorage.clear();
    sessionStorage.clear();
  });
  await page.goto("/tests/frontend/builtin-plugins.html?mode=rejected");
  await review(page);
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "待核查的插件操作" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("switch", { name: "启用 Codex 本机来源", exact: true }),
  ).toBeEnabled();
});
test("窄窗键盘核对可退出，损坏恢复记录阻止新的写入", async ({ page }) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/builtin-plugins.html");
  await review(page);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.evaluate(
    (p) => localStorage.setItem(p + "corrupt", "{broken"),
    prefix,
  );
  await page.reload();
  await expect(page.getByText("恢复记录不可用", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("switch", { name: "启用 Codex 本机来源", exact: true }),
  ).toBeDisabled();
});

test("暂停后可单独恢复所需支撑，不使用技术模式选择，不开启无关功能", async ({
  page,
}) => {
  await page.goto("/tests/frontend/builtin-plugins.html?mode=recovery");
  const source = page.getByRole("switch", {
    name: "启用 Codex 本机来源",
    exact: true,
  });
  await expect(source).not.toBeChecked();
  await expect(source).toBeEnabled();
  await expect(
    page.getByRole("combobox", { name: "插件组合", exact: true }),
  ).toHaveCount(0);
  await source.click();
  await expect(page.getByRole("dialog")).toContainText("Codex 本机来源 · 启用");
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect(source).not.toBeChecked();
  expect(
    await page.evaluate(() =>
      window.builtinCalls.filter(
        (c) => c.command === "configure_builtin_plugins",
      ),
    ),
  ).toEqual([]);
  await source.click();
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(source).toBeChecked();
  await expect(
    page.getByRole("switch", { name: "启用 项目来源读取", exact: true }),
  ).not.toBeChecked();
});

test("运行异常不能显示为开启，开关暂停操作并保留整组恢复入口", async ({
  page,
}) => {
  await page.goto("/tests/frontend/builtin-plugins.html?mode=degraded");
  const source = page.getByRole("switch", {
    name: "启用 Codex 本机来源",
    exact: true,
  });
  await expect(source).not.toBeChecked();
  await expect(source).toBeDisabled();
  await expect(page.getByText("运行异常", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "恢复应用功能", exact: true }),
  ).toBeEnabled();
});

test("采集器停止后汇总保持未知，历史活动不再冒充当前执行", async ({ page }) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&collector=stopped#/overview",
  );
  await page.evaluate(() => {
    window.activityCase = "empty";
  });
  const summary = page.getByRole("region", {
    name: "实时任务汇总",
    exact: true,
  });
  await expect(summary).toContainText(
    "原生活动采集已暂停，当前执行状态待确认。",
  );
  await expect(
    summary.getByRole("link", { name: "管理活动采集", exact: true }),
  ).toHaveAttribute("href", "#/plugins?scene=activity");
  await expect(summary).not.toContainText("当前没有观测到正在执行的任务。");
  await page.evaluate(() => {
    window.activityCase = "native-desktop-live";
  });
  await expect(
    page.getByRole("region", { name: "当前活动列表", exact: true }),
  ).toContainText("状态待更新");
  await expect(summary).toContainText(
    "原生活动采集已暂停，当前执行状态待确认。",
  );
});

test("最小窗口核对全部插件时列表内部滚动，确认与取消始终可见", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/builtin-plugins.html?mode=recovery");
  await page.getByRole("button", { name: "恢复应用功能", exact: true }).click();
  const dialog = page.getByRole("dialog");
  const confirm = dialog.getByRole("button", {
    name: "确认变更",
    exact: true,
  });
  await expect(confirm).toBeInViewport({ ratio: 1 });
  await expect(
    dialog.getByRole("button", { name: "取消", exact: true }),
  ).toBeInViewport({ ratio: 1 });
  await expect(
    dialog.getByRole("heading", { name: "确认插件变更", exact: true }),
  ).toBeInViewport({ ratio: 1 });
  await confirm.focus();
  await expect(confirm).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        window.builtinCalls.filter(
          (c) => c.command === "configure_builtin_plugins",
        ).length,
    ),
  ).toBe(0);
});

test("恢复桌面组合后已打开的业务立即重新读取，不等待计时器或重开页面", async ({
  page,
}) => {
  await page.goto("/tests/frontend/builtin-plugins.html?mode=recovery");
  const business = page.getByRole("region", {
    name: "业务读取状态",
    exact: true,
  });
  await expect(business).toContainText("应用功能暂时暂停");
  await page.getByRole("button", { name: "恢复应用功能", exact: true }).click();
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "插件变更已保存" }),
  ).toBeVisible();
  await expect(business).toContainText("桌面业务已恢复读取");
  expect(
    await page.evaluate(
      () =>
        window.builtinCalls.filter(
          (c) => c.command === "configure_builtin_plugins",
        ).length,
    ),
  ).toBe(1);
});

test("恢复组合在所有页面明确标示业务暂停，并能定位官方组合恢复入口", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&composition=recovery#/overview",
  );
  const notice = page.getByLabel("功能可用状态", { exact: true });
  await expect(notice).toContainText("应用功能已暂停");
  await expect(notice).toContainText("部分功能暂时无法使用");
  await notice
    .getByRole("button", { name: "打开插件中心", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/plugins$/);
  await expect(
    page.getByRole("heading", { name: "插件中心", exact: true }).last(),
  ).toBeVisible();
  await expect(page.getByText("运行组合", { exact: true })).toHaveCount(0);
  await expect(
    page.getByRole("group", { name: "功能场景", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("tab", { name: "官方组合", exact: true }),
  ).toHaveCount(0);
});

test("后台组合提示采集暂停，桌面默认组合不显示业务暂停提示", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&composition=background#/overview",
  );
  await expect(page.getByLabel("功能可用状态", { exact: true })).toContainText(
    "其他功能入口继续可用",
  );
  await page.goto("/tests/frontend/tables.html?mode=activity#/overview");
  await expect(page.getByLabel("功能可用状态", { exact: true })).toHaveCount(0);
});

test("恢复自动更新保留用户停用的其他功能，不暴露技术运行模式", async ({
  page,
}) => {
  await page.goto("/tests/frontend/builtin-plugins.html?mode=background");
  const source = page.getByRole("switch", {
    name: "启用 Codex 本机来源",
    exact: true,
  });
  await expect(source).not.toBeChecked();
  await page.getByRole("button", { name: "恢复自动更新", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("实时活动采集 · 启用");
  await page.getByRole("button", { name: "确认变更", exact: true }).click();
  await expect(
    page.getByRole("switch", { name: "启用 实时活动采集", exact: true }),
  ).toBeChecked();
  await expect(source).not.toBeChecked();
  const changes = await page.evaluate(() =>
    window.builtinCalls.filter(
      (call) => call.command === "configure_builtin_plugins",
    ),
  );
  expect(changes).toHaveLength(1);
  expect(changes[0].input).toMatchObject({
    composition: { profile: "desktop", disabledPluginIds: ["source.codex"] },
  });
});
