import { expect, test } from "@playwright/test";
import { PROVIDER_FIXTURE_STORE_ID } from "./provider-operation-fixture";

test("provider account inspection verifies the selected model and displays stored cooldowns", async ({
  page,
}) => {
  await page.setViewportSize({ width: 780, height: 650 });
  await page.goto(
    "/tests/frontend/tables.html?mode=upstream-account-verification",
  );
  await page.getByRole("tab", { name: "上游账号", exact: true }).click();
  const row = page.getByRole("row").filter({ hasText: "验证账号" });
  await expect(row).toContainText("验证已过期");
  await expect(row).toContainText("有候选处于冷却");
  await row.getByRole("button", { name: "验证与状态 验证账号" }).click();
  const dialog = page.getByRole("dialog", { name: "验证与状态 · 验证账号" });
  await expect(dialog.getByText(/上游限流/)).toBeVisible();
  const model = dialog.getByRole("combobox", { name: "验证模型" });
  await model.fill("model-324");
  await page.getByRole("option", { name: "model-324", exact: true }).click();
  await dialog.getByRole("button", { name: "开始验证" }).click();
  await expect(dialog.getByRole("status")).toHaveText("model-324 验证通过。");
  await expect(
    dialog.getByRole("region", { name: "模型验证记录" }),
  ).toContainText("model-324");
  expect(
    await page.evaluate(() =>
      window.scopeQueries
        .filter((q) => q.feature === "upstream-account")
        .map((q) => q.query),
    ),
  ).toEqual([
    {
      kind: "verify",
      storeId: PROVIDER_FIXTURE_STORE_ID,
      expectedRevision: 1,
      account: { id: "00000000-0000-4000-8000-000000000101", revision: 3 },
      model: "model-324",
    },
  ]);
  await dialog.getByRole("button", { name: "关闭", exact: true }).click();
  await expect(row).toContainText("model-324");
  await page.getByRole("button", { name: "验证与状态 暂停账号" }).click();
  const paused = page.getByRole("dialog", { name: "验证与状态 · 暂停账号" });
  await expect(paused.getByText("已暂停", { exact: true })).toBeVisible();
  await expect(paused.getByRole("button", { name: "开始验证" })).toHaveCount(0);
});

test("Agent source state distinguishes partial transcripts from read failure with cached sessions", async ({
  page,
}) => {
  for (const [state, label] of [
    ["ready", "历史会话已接入"],
    ["partial", "部分会话可读"],
    ["failed", "会话读取异常"],
  ]) {
    for (const name of ["OpenCode CLI", "OpenCode Desktop"]) {
      await page.goto(
        `/tests/frontend/tables.html?mode=agents&sourceState=${state}#/agents`,
      );
      const sourceRow = page.getByRole("row").filter({
        has: page.getByRole("link", { name: `详情 ${name}`, exact: true }),
      });
      await expect(sourceRow).toContainText(label);
      if (state === "partial")
        await expect(sourceRow).not.toContainText("会话读取异常");
      await sourceRow
        .getByRole("link", { name: `详情 ${name}`, exact: true })
        .click();
      await expect(
        page.getByText(label, { exact: true }).first(),
      ).toBeVisible();
    }
  }
});

test("native Agent rows navigate to their own details alongside bundled DSH", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/tables.html?mode=agents#/agents");
  const names = [
    "Claude Code CLI",
    "Claude Code 桌面端",
    "Codex CLI",
    "Codex 桌面端",
    "OpenCode CLI",
    "OpenCode Desktop",
  ];
  const ids = [
    "claude-code-cli",
    "claude-code-desktop",
    "codex-cli",
    "codex-desktop",
    "opencode-cli",
    "opencode-desktop",
  ];
  for (const name of names)
    await expect(
      page.getByRole("link", { name: `详情 ${name}`, exact: true }),
    ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "详情 Claude Code CLI", exact: true }),
  ).toBeInViewport();
  await expect(page.getByRole("link", { name: /^详情 / })).toHaveCount(7);
  await expect(
    page.getByRole("link", { name: "详情 DeepSeek Harness 桌面端" }),
  ).toBeVisible();
  for (let i = 0; i < ids.length; i++) {
    await page
      .getByRole("link", { name: /^详情 / })
      .nth(i)
      .click();
    await expect(page).toHaveURL(new RegExp(`/agents/${ids[i]}$`));
    await expect(
      page.getByRole("heading", { name: names[i], exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("tab")).toHaveCount(6);
    await expect(
      page.getByRole("tab", { name: "记忆", exact: true }),
    ).toBeVisible();
    if (ids[i].startsWith("codex-"))
      await expect(page.getByText(/CLI 与桌面共用 Codex 配置/)).toBeVisible();
    await page.getByRole("link", { name: "返回 Agents", exact: true }).click();
  }
});
import type {} from "./tables-harness";

test("MCP independent Agent filters retain configuration-only groups and scope detail requests", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=mcp-page#/mcp");
  await page.getByRole("combobox", { name: "MCP Agent", exact: true }).click();
  for (const name of [
    "Claude Code CLI",
    "Claude Code 桌面端",
    "Codex CLI",
    "Codex 桌面端",
    "OpenCode CLI",
    "OpenCode Desktop",
    "客户端未知",
  ])
    await expect(page.getByRole("option", { name, exact: true })).toBeVisible();
  await page.getByRole("option", { name: "Codex CLI", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries.filter((q) => q.feature === "mcp").at(-1)?.query
            .agent,
      ),
    )
    .toBe("codex-cli");
  await expect(
    page.getByRole("region", { name: "MCP 聚合列表" }),
  ).toContainText("配置适用：Codex CLI / Codex 桌面端");
  await page
    .getByRole("button", { name: "查看 Fixture MCP", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "MCP 聚合详情" }),
  ).toBeVisible();
  await expect(page).toHaveURL(/agent=codex-cli/);
  await page.getByRole("combobox", { name: "MCP Agent", exact: true }).click();
  await page.getByRole("option", { name: "客户端未知", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries.filter((q) => q.feature === "mcp").at(-1)?.query
            .agent,
      ),
    )
    .toBe("unknown");
  await expect(
    page.getByRole("region", { name: "MCP 聚合详情" }),
  ).toContainText("调用来源：客户端来源未知");
});

test("project Agent filter drives totals, calls, date and tab navigation together", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=projects-page#/projects");
  await page.getByRole("combobox", { name: "项目 Agent", exact: true }).click();
  await page.getByRole("option", { name: "Codex CLI", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries.filter((q) => q.feature === "projects").at(-1)
            ?.query.agent,
      ),
    )
    .toBe("codex-cli");
  await page.getByRole("button", { name: "查看项目", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "项目 Agent 用量" }),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries.filter((q) => q.feature === "calls").at(-1)
            ?.query,
      ),
    )
    .toMatchObject({ agent: "codex-cli", project: "fixture-project" });
  await page
    .getByRole("combobox", { name: "项目历史日期", exact: true })
    .click();
  await page.getByRole("option", { name: "近 7 天", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries.filter((q) => q.feature === "project").at(-1)
            ?.query,
      ),
    )
    .toMatchObject({ agent: "codex-cli", days: 7 });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries.filter((q) => q.feature === "calls").at(-1)
            ?.query,
      ),
    )
    .toMatchObject({ agent: "codex-cli", days: 7 });
  await page.getByRole("tab", { name: "Sessions", exact: true }).click();
  await expect(page).toHaveURL(/agent=codex-cli/);
  await page.getByRole("combobox", { name: "项目 Agent", exact: true }).click();
  await page.getByRole("option", { name: "客户端未知", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries.filter((q) => q.feature === "project").at(-1)
            ?.query.agent,
      ),
    )
    .toBe("unknown");
  await page.getByRole("tab", { name: "总览", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Agent 调用记录", exact: true }),
  ).toContainText("Codex · 客户端来源未知");
});

test("project defaults to regular workspaces and temporary directories remain separately accessible", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=projects-page#/projects");
  await expect(
    page.getByRole("heading", { name: "Fixture Project", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Temporary fixture", exact: true }),
  ).toHaveCount(0);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.scopeQueries.filter((q) => q.feature === "projects").at(-1)
            ?.query.scope,
      ),
    )
    .toBe("workspace");
  await page.getByRole("combobox", { name: "项目收录范围" }).click();
  await page
    .getByRole("option", { name: "临时与验证目录", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Temporary fixture", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Fixture Project", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("option", { name: "临时与验证目录", exact: true }),
  ).toBeHidden();
  await page.getByRole("combobox", { name: "项目收录范围" }).click();
  await page.getByRole("option", { name: "常规项目", exact: true }).click();
  await page.getByRole("button", { name: "查看项目", exact: true }).click();
  const picker = page.getByRole("combobox", { name: "切换项目", exact: true });
  await picker.fill("Fixture");
  await expect(
    page.getByRole("option", { name: /Fixture Project/ }),
  ).toBeVisible();
  await picker.press("ArrowDown");
  await picker.press("Enter");
  await expect(
    page.getByRole("region", { name: "项目 Agent 用量", exact: true }),
  ).toBeVisible();
});

test("project visibility supports cancellation, failure, hiding across scopes and explicit recovery", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=projects-page#/projects");
  await page
    .getByRole("button", { name: "隐藏项目 Fixture Project", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText(
    "原始 Session、调用、历史用量和磁盘文件都保留",
  );
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  expect(
    await page.evaluate(() => window.projectVisibilityChanges.length),
  ).toBe(0);
  await expect(dialog).toBeHidden();
  await page
    .getByRole("button", { name: "隐藏项目 Fixture Project", exact: true })
    .click();
  await page.evaluate(() => {
    window.failProjectVisibility = true;
  });
  await dialog.getByRole("button", { name: "隐藏项目", exact: true }).click();
  await expect(dialog).toContainText("显示设置保存失败");
  await expect(
    page.getByRole("heading", {
      name: "Fixture Project",
      exact: true,
      includeHidden: true,
    }),
  ).toHaveCount(1);
  await page.evaluate(() => {
    window.failProjectVisibility = false;
  });
  await dialog.getByRole("button", { name: "隐藏项目", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(
    page.getByRole("heading", { name: "Fixture Project", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "已隐藏项目 · 1", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("combobox", { name: "项目收录范围", exact: true })
    .click();
  await page.getByRole("option", { name: "全部会话目录", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Temporary fixture", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Fixture Project", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "查看项目", exact: true }).click();
  await page.getByRole("button", { name: "展开项目目录", exact: true }).click();
  await expect(
    page.getByRole("option", { name: /Fixture Project/ }),
  ).toHaveCount(0);
  await page.getByRole("option", { name: /Temporary fixture/ }).click();
  await expect(
    page.getByRole("combobox", { name: "切换项目", exact: true }),
  ).toHaveAttribute("aria-expanded", "false");
  await page
    .getByRole("button", { name: "已隐藏项目 · 1", exact: true })
    .click();
  await expect(
    dialog.getByRole("region", { name: "已隐藏项目列表", exact: true }),
  ).toContainText("Fixture Project");
  await dialog
    .getByRole("textbox", { name: "搜索已隐藏项目", exact: true })
    .fill("/tmp/fixture");
  await dialog
    .getByRole("button", { name: "恢复项目 Fixture Project", exact: true })
    .click();
  await expect(dialog).toContainText("没有已隐藏项目");
  await dialog.getByRole("button", { name: "关闭", exact: true }).click();
  await expect(dialog).toBeHidden();
  await page.getByRole("button", { name: "展开项目目录", exact: true }).click();
  await expect(
    page.getByRole("option", { name: /Fixture Project/ }),
  ).toBeVisible();
});

test("hidden project bookmarks show recovery notice without returning the project to the picker", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=projects-page#/projects/fixture-project",
  );
  await page
    .getByRole("button", { name: "隐藏项目 Fixture Project", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "隐藏项目", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/projects$/);
  await page.evaluate(() => {
    location.hash = "/projects/fixture-project";
  });
  await expect(
    page.getByText("此项目已隐藏", { exact: true }).first(),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "切换项目", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("region", { name: "项目 Agent 用量", exact: true }),
  ).toHaveCount(0);
});

test("MCP hidden project bookmarks require an explicit filter reset", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=mcp-page#/mcp?project=hidden-project",
  );
  await expect(page.getByRole("alert")).toContainText("所选项目已隐藏");
  await expect(
    page.getByRole("region", { name: "MCP 聚合列表", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "清除项目筛选", exact: true }).click();
  await expect(page).toHaveURL(/#\/mcp$/);
  await expect(
    page.getByRole("region", { name: "MCP 聚合列表", exact: true }),
  ).toBeVisible();
});

test("provider records use localized DataTable pagination and sorting", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=providers");
  const region = page.getByRole("region", { name: "供应商连接", exact: true });
  await expect(region.getByRole("row")).toHaveCount(21);
  await expect(region.getByText("第 1 / 3 页")).toBeVisible();
  await region.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(region.getByRole("row").nth(1)).toContainText("连接 020");
  await region.getByRole("button", { name: "第一页", exact: true }).click();
  await region.getByRole("button", { name: "供应商", exact: true }).click();
  await page.getByText("降序", { exact: true }).click();
  await expect(region.getByRole("row").nth(1)).toContainText("连接 044");
});

test("model pagination, filtering and immediate switches preserve the full catalog", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html");
  await page.getByRole("tab", { name: "模型", exact: true }).click();
  const region = page.getByRole("region", {
    name: "供应商模型目录",
    exact: true,
  });
  await expect(region.getByRole("row")).toHaveCount(51);
  await region.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(region.getByRole("row").nth(1)).toContainText("model-050");
  await page
    .getByRole("textbox", { name: "搜索模型 ID", exact: true })
    .fill("model-324");
  await expect(region.getByRole("row")).toHaveCount(2);
  await expect(region.getByText("第 1 / 1 页")).toBeVisible();
  await region
    .getByRole("switch", { name: "启用模型 model-324", exact: true })
    .click();
  await expect(
    region.getByRole("switch", { name: "启用模型 model-324", exact: true }),
  ).not.toBeChecked();
  expect(
    await page.evaluate(() => window.savedProviders.at(-1)?.models.length),
  ).toBe(325);
  expect(
    await page.evaluate(() => window.savedProviders.at(-1)?.disabledModels),
  ).toEqual(["model-324"]);
  await page.getByRole("button", { name: "清除搜索", exact: true }).click();
  await expect(region.getByRole("row")).toHaveCount(51);
  await expect(region.getByText("第 1 / 7 页")).toBeVisible();
});

test("model picker searches beyond its 200 visible options and saves the selected ID", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html");
  await page.getByRole("button", { name: "配置连接", exact: true }).click();
  await page.getByRole("button", { name: "高级设置", exact: true }).click();
  const picker = page.getByRole("combobox", { name: "默认模型", exact: true });
  await picker.fill("model-324");
  await expect(
    page.getByRole("option", { name: "model-324", exact: true }),
  ).toBeVisible();
  await picker.press("ArrowDown");
  await picker.press("Enter");
  await expect(picker).toHaveValue("model-324");
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.savedProviders.at(-1)?.model))
    .toBe("model-324");
});

test("cursor data remains complete and initial loading uses Zeron skeleton states", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=requests");
  const region = page.getByRole("region", { name: "请求", exact: true });
  await expect(region.getByRole("row")).toHaveCount(151);
  await expect(
    region.getByRole("button", { name: "下一页", exact: true }),
  ).toHaveCount(0);
  await page.goto("/tests/frontend/tables.html?mode=loading");
  await expect(page.getByRole("status")).toHaveText("正在读取调用记录");
});

test("DataTable owns horizontal overflow at narrow width", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 780 });
  await page.goto("/tests/frontend/tables.html?mode=providers");
  await expect(
    page.getByRole("region", { name: "供应商连接", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(page.getByText("第 2 / 3 页")).toBeVisible();
});

test("MCP rows activate by keyboard while nested session actions keep their own behavior", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=mcp");
  const region = page.getByRole("region", {
    name: "MCP 调用列表",
    exact: true,
  });
  const row = region.getByRole("row").nth(1);
  await row.focus();
  await row.press("Enter");
  await expect(
    page.getByRole("region", { name: "MCP 调用详情", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "调用详情 · read", exact: true }),
  ).toBeVisible();
  await row.focus();
  await row.press("Enter");
  await expect(
    page.getByRole("region", { name: "MCP 调用详情", exact: true }),
  ).toHaveCount(0);
  await region
    .getByRole("button", { name: "查看 Session", exact: true })
    .click();
  await expect(page).toHaveURL(/#\/sessions\/fixture-session\?/);
  await expect(
    page.getByRole("region", { name: "MCP 调用详情", exact: true }),
  ).toHaveCount(0);
});

test("a refreshed smaller collection clamps the current page without hiding remaining records", async ({
  page,
}) => {
  await page.goto("/tests/frontend/tables.html?mode=providers");
  const region = page.getByRole("region", { name: "供应商连接", exact: true });
  await region.getByRole("button", { name: "最后一页", exact: true }).click();
  await expect(region.getByText("第 3 / 3 页")).toBeVisible();
  await page.getByRole("button", { name: "模拟目录减少", exact: true }).click();
  await expect(region.getByText("第 1 / 1 页")).toBeVisible();
  await expect(region.getByRole("row")).toHaveCount(4);
});

test("the app shell keeps header and document fixed while the page body scrolls; sidebar uses the public shortcut", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/tables.html?mode=shell#/providers/fixture-0",
  );
  await page.getByRole("tab", { name: "模型", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "供应商模型目录", exact: true }),
  ).toBeVisible();
  const banner = page.getByRole("banner");
  const before = await banner.boundingBox();
  await page
    .getByRole("region", { name: "供应商模型目录", exact: true })
    .hover();
  await page.mouse.wheel(0, 700);
  await expect
    .poll(async () => (await banner.boundingBox())?.y)
    .toBe(before?.y);
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
  await page.getByRole("heading", { name: "模型供应商", exact: true }).click();
  await page.keyboard.press("ControlOrMeta+b");
  await expect(
    page.getByRole("link", { name: "总览", exact: true }),
  ).not.toBeInViewport();
  await page.keyboard.press("ControlOrMeta+b");
  await expect(
    page.getByRole("link", { name: "总览", exact: true }),
  ).toBeVisible();
});

for (const [width, height] of [
  [1280, 860],
  [720, 560],
] as const)
  for (const dark of [false, true])
    test(`long settings keep document scroll fixed at ${width}x${height} in ${dark ? "dark" : "light"} mode`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height });
      await page.goto("/tests/frontend/tables.html?mode=shell#/settings");
      await page.evaluate((enabled) => {
        document.documentElement.classList.toggle("dark", enabled);
        document.documentElement.dataset.theme = enabled ? "dark" : "light";
      }, dark);
      await expect(
        page.getByRole("heading", { name: "待开放能力", exact: true }),
      ).toBeAttached();
      await expect(
        page.getByRole("region", { name: "本地已安装插件管理", exact: true }),
      ).toHaveCount(0);
      await expect(
        page.getByRole("combobox", { name: "记录保留期", exact: true }),
      ).toBeDisabled();
      await expect(
        page.getByRole("button", { name: "保存偏好", exact: true }),
      ).toBeDisabled();
      const body = page.locator('[data-slot="page-body"]');
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollHeight))
        .toBeLessThanOrEqual(height + 1);
      expect(await body.evaluate((e) => e.scrollHeight > e.clientHeight)).toBe(
        true,
      );
      // Table announcements remain in the accessibility tree. Removing the
      // offscreen status would hide the overflow while breaking accessibility.
      const announcements = body.locator(
        '[data-slot="data-table"] > [role="status"]',
      );
      expect(await announcements.count()).toBeGreaterThan(0);
      await expect(announcements.first()).toHaveAttribute(
        "aria-atomic",
        "true",
      );
      const banner = page.getByRole("banner");
      const bannerBefore = await banner.boundingBox();
      await page.evaluate(() =>
        window.scrollTo(0, document.documentElement.scrollHeight),
      );
      await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(0);
      await page
        .getByRole("button", { name: "刷新诊断记录", exact: true })
        .focus();
      await page.keyboard.press("PageDown");
      await expect
        .poll(() => body.evaluate((e) => e.scrollTop))
        .toBeGreaterThan(0);
      expect(await page.evaluate(() => window.scrollY)).toBe(0);
      expect((await banner.boundingBox())?.y).toBe(bannerBefore?.y);
      // Exercise the settings page's read-only diagnostics table. Plugin
      // management has its own page; retention writes remain unauthorized.
      await page
        .getByRole("button", { name: "最近诊断记录", exact: true })
        .click();
      const diagnostics = page.getByLabel("最近后台连接诊断", { exact: true });
      await expect(diagnostics.getByRole("row")).toHaveCount(11);
      const pageSize = diagnostics.getByRole("combobox", {
        name: "每页条数",
        exact: true,
      });
      await pageSize.click();
      await expect(
        page.getByRole("option", { name: "20", exact: true }),
      ).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(
        page.getByRole("option", { name: "20", exact: true }),
      ).toHaveCount(0);
      await expect(pageSize).toBeFocused();
      await pageSize.click();
      await page.getByRole("option", { name: "20", exact: true }).click();
      await expect(diagnostics.getByRole("row")).toHaveCount(21);
      await expect(diagnostics.getByText("第 1 / 2 页")).toBeVisible();
      await diagnostics
        .getByRole("button", { name: "下一页", exact: true })
        .click();
      await expect(diagnostics.getByRole("row")).toHaveCount(13);
      await expect(diagnostics.getByText("第 2 / 2 页")).toBeVisible();
      expect(await page.evaluate(() => window.scrollY)).toBe(0);
      expect((await banner.boundingBox())?.y).toBe(bannerBefore?.y);
    });

test("background snapshots preserve an open DataTable sort menu", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto(
    "/tests/frontend/tables.html?mode=shell#/providers/fixture-0",
  );
  await page.getByRole("tab", { name: "模型", exact: true }).click();
  const table = page.getByRole("region", {
    name: "供应商模型目录",
    exact: true,
  });
  await table.getByRole("button", { name: "模型 ID", exact: true }).click();
  await expect(page.getByText("降序", { exact: true })).toBeVisible();
  await page.clock.fastForward(5_000);
  await expect(page.getByText("降序", { exact: true })).toBeVisible();
  await page.getByText("降序", { exact: true }).click();
  await expect(table.getByRole("row").nth(1)).toContainText("model-324");
});
