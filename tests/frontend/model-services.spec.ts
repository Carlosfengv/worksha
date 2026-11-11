import { expect, test } from "@playwright/test";

test("publishes explicit access fallback and restores it in the editor", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  const edit = page.getByRole("button", { name: "编辑 编程服务", exact: true });
  await edit.click();
  await page.getByRole("switch", { name: "启用优先级与自动回退" }).click();
  const access = page.getByRole("switch", {
    name: "上游明确拒绝凭据或模型权限时回退",
  });
  await expect(access).not.toBeChecked();
  await access.click();
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        window.modelServiceFixture.catalog.services[0].routingPolicy
          ?.fallbackOnAccessDenied,
    ),
  ).toBe(true);
  await edit.click();
  await expect(access).toBeChecked();
  await access.click();
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await edit.click();
  await expect(access).not.toBeChecked();
});

test("publishes and reopens a child service pinned to its chosen version", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?nested=1");
  await page.getByRole("button", { name: "创建模型服务", exact: true }).click();
  await page.getByRole("textbox", { name: "服务名称" }).fill("父服务");
  await page.getByRole("button", { name: "添加子服务", exact: true }).click();
  const publish = page.getByRole("button", { name: "验证并发布" });
  await expect(publish).toBeDisabled();
  const child = page.getByRole("combobox", { name: "子服务版本" });
  await child.click();
  await page.getByRole("option", { name: "编程服务 · 第 1 版" }).click();
  await expect(
    page.getByRole("spinbutton", { name: "上下文窗口（Token）" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("switch", { name: "启用优先级与自动回退" }),
  ).toBeChecked();
  await expect(
    page.getByRole("combobox", { name: "添加上游账号" }),
  ).toHaveCount(0);
  await publish.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const saved = await page.evaluate(() => ({
    actions: window.modelServiceFixture.actions,
    child: window.modelServiceFixture.catalog.services.find(
      (service) => service.name === "父服务",
    )?.models[0].child,
  }));
  expect(saved.actions).toEqual(["save"]);
  expect(saved.child).toEqual({
    serviceId: "620d7132-b3f3-4e56-8f5a-2c06c8282a01",
    versionId: "70130e70-ef47-4cd7-9b01-2da76b6101d5",
  });
  await page.getByRole("button", { name: "编辑 父服务", exact: true }).click();
  await expect(child).toHaveValue("编程服务 · 第 1 版");
});

test("publishes an opted-in intent rule with a fixed classifier account", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?upstream-accounts=1");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page.getByRole("switch", { name: "启用优先级与自动回退" }).click();
  const classifier = page.getByRole("switch", { name: "启用意图分类" });
  await expect(classifier).not.toBeChecked();
  await page.getByRole("combobox", { name: "添加上游账号" }).click();
  await page.getByRole("option", { name: "主账号", exact: true }).click();
  await classifier.click();
  await page.getByRole("combobox", { name: "分类模型" }).click();
  await page.getByRole("option", { name: "Pro", exact: true }).click();
  const publish = page.getByRole("button", { name: "验证并发布", exact: true });
  await expect(publish).toBeDisabled();
  await page.getByRole("combobox", { name: "分类账号" }).click();
  await page.getByRole("option", { name: "主账号", exact: true }).click();
  await page.getByRole("button", { name: "添加规则" }).click();
  await page.getByRole("combobox", { name: "规则目标模型" }).click();
  await page.getByRole("option", { name: "Flash", exact: true }).click();
  await page.getByRole("textbox", { name: "请求意图" }).press("End");
  await page.keyboard.insertText("排查错误");
  await publish.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const policy = await page.evaluate(
    () => window.modelServiceFixture.catalog.services[0].routingPolicy,
  );
  expect(policy?.intentClassifier).toMatchObject({
    modelId: "model-pro",
    account: { id: "a8259fd0-66d1-493f-ac6f-12abc5437890", revision: 1 },
  });
  expect(policy?.rules?.[0]).toMatchObject({
    modelId: "model-flash",
    intent: "排查错误",
  });
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await expect(page.getByRole("combobox", { name: "分类账号" })).toContainText(
    "主账号",
  );
  await classifier.click();
  await publish.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const disabledPolicy = await page.evaluate(
    () => window.modelServiceFixture.catalog.services[0].routingPolicy,
  );
  expect(disabledPolicy?.intentClassifier).toBeUndefined();
  expect(disabledPolicy?.rules?.[0].intent).toBe("排查错误");
});

test("publishes ordered conditions and restores them in the editor", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page.getByRole("switch", { name: "启用优先级与自动回退" }).click();
  await page.getByRole("button", { name: "添加规则" }).click();
  const name = page.getByRole("textbox", { name: "规则名称" });
  await expect(name).toBeFocused();
  await name.press("ControlOrMeta+a");
  await page.keyboard.insertText("OpenCode 工具");
  await page.getByRole("combobox", { name: "规则目标模型" }).click();
  await page.getByRole("option", { name: "Flash", exact: true }).click();
  const publish = page.getByRole("button", { name: "验证并发布", exact: true });
  await expect(publish).toBeDisabled();
  await page.getByRole("combobox", { name: "客户端条件" }).click();
  await expect(
    page.getByRole("option", { name: "OpenCode · 全部客户端", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("option", { name: "OpenCode Desktop", exact: true }),
  ).toBeVisible();
  await page.getByRole("option", { name: "OpenCode CLI", exact: true }).click();
  await page.getByRole("combobox", { name: "工具定义" }).click();
  await page.getByRole("option", { name: "包含可用工具", exact: true }).click();
  const tokens = page.getByRole("spinbutton", { name: /最低输入 Token 数/ });
  await tokens.press("2");
  await page.keyboard.insertText("048");
  await page.getByRole("button", { name: "添加规则" }).click();
  await expect(name).toBeFocused();
  await name.press("ControlOrMeta+a");
  await page.keyboard.insertText("流式聊天");
  await page.getByRole("combobox", { name: "规则目标模型" }).click();
  await page.getByRole("option", { name: "Pro", exact: true }).click();
  await page.getByRole("combobox", { name: "响应方式" }).click();
  await page.getByRole("option", { name: "流式", exact: true }).click();
  await page.getByRole("button", { name: "上移规则" }).click();
  await publish.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const rules = await page.evaluate(
    () => window.modelServiceFixture.catalog.services[0].routingPolicy?.rules,
  );
  expect(rules).toMatchObject([
    { name: "流式聊天", modelId: "model-pro", stream: true },
    {
      name: "OpenCode 工具",
      modelId: "model-flash",
      agent: "opencode-cli",
      tools: true,
      minInputTokens: 2048,
    },
  ]);
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("button", { name: "OpenCode 工具", exact: true })
    .click();
  await expect(
    page.getByRole("spinbutton", { name: /最低输入 Token 数/ }),
  ).toHaveValue("2048");
  await expect(
    page.getByRole("combobox", { name: "客户端条件" }),
  ).toContainText("OpenCode CLI");
});

test("selects and republishes a project available only to routing", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  const edit = page.getByRole("button", { name: "编辑 编程服务", exact: true });
  await edit.click();
  await page.getByRole("switch", { name: "启用优先级与自动回退" }).click();
  await page.getByRole("button", { name: "添加规则" }).click();
  const ruleName = page.getByRole("textbox", { name: "规则名称" });
  await ruleName.fill("OpenCode 2.x 项目规则");
  await page.getByRole("combobox", { name: "规则目标模型" }).click();
  await page.getByRole("option", { name: "Flash", exact: true }).click();
  await page.getByRole("button", { name: "选择项目", exact: true }).click();
  const project = page.getByRole("combobox", { name: "项目条件" });
  await project.fill("opencode-v2");
  await page.getByRole("option", { name: /OpenCode 2.x 项目/ }).click();
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        window.modelServiceFixture.catalog.services[0].routingPolicy?.rules?.[0]
          ?.projectId,
    ),
  ).toBe(`project:${"2".repeat(64)}`);
  await edit.click();
  await page
    .getByRole("button", { name: "OpenCode 2.x 项目规则", exact: true })
    .click();
  await expect(project).toHaveValue(/OpenCode 2.x 项目/);
  await page.evaluate(() => window.modelServiceFixture.revokeRoutingProject());
  await expect(project).toHaveValue("所选项目暂不可用");
});

test("publish upstream account order, weights and affinity", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?upstream-accounts=1");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page.getByRole("switch", { name: "启用优先级与自动回退" }).click();
  const picker = page.getByRole("combobox", { name: "添加上游账号" });
  await picker.click();
  await expect(page.getByRole("option", { name: "暂停账号" })).toHaveCount(0);
  await page.getByRole("option", { name: "主账号" }).click();
  await picker.click();
  await page.getByRole("option", { name: "备用账号" }).click();
  await page.getByRole("button", { name: "上移 备用账号" }).click();
  await page.getByRole("combobox", { name: "账号选择策略" }).click();
  await page.getByRole("option", { name: "按权重分配" }).click();
  const weight = page.getByRole("spinbutton", { name: "备用账号 权重" });
  await weight.fill("0");
  await expect(page.getByRole("button", { name: "验证并发布" })).toBeDisabled();
  await weight.fill("3");
  await page.getByRole("combobox", { name: "同会话后续轮次" }).click();
  await page.getByRole("option", { name: "优先沿用" }).click();
  await expect(
    page.getByRole("switch", { name: "账号池耗尽后允许跨模型回退" }),
  ).not.toBeChecked();
  await page.getByRole("button", { name: "验证并发布" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const state = await page.evaluate(() => ({
    actions: window.modelServiceFixture.actions,
    pool: window.modelServiceFixture.catalog.services[0].routingPolicy
      ?.accountPools?.[0],
  }));
  expect(state.actions).toEqual([
    "account:b8259fd0-66d1-493f-ac6f-12abc5437890",
    "verify:source-Alpha:a",
    "account:a8259fd0-66d1-493f-ac6f-12abc5437890",
    "verify:source-Alpha:a",
    "save",
  ]);
  expect(state.pool?.accounts.map((a) => a.id)).toEqual([
    "b8259fd0-66d1-493f-ac6f-12abc5437890",
    "a8259fd0-66d1-493f-ac6f-12abc5437890",
  ]);
  expect(state.pool).toMatchObject({
    strategy: "weighted",
    affinity: "prefer",
    weights: { "b8259fd0-66d1-493f-ac6f-12abc5437890": 3 },
  });
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page.getByRole("combobox", { name: "账号选择策略" }).click();
  await page
    .getByRole("option", { name: "优先用每小时剩余额度比例高的账号" })
    .click();
  await expect(page.getByText(/额度排序当前使用 OpenRouter/)).toBeVisible();
  await expect(page.getByText(/按到重置前的剩余时间折算/)).toBeVisible();
  await page.getByRole("button", { name: "验证并发布" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const quotaPool = await page.evaluate(
    () =>
      window.modelServiceFixture.catalog.services[0].routingPolicy
        ?.accountPools?.[0],
  );
  expect(quotaPool?.strategy).toBe("quota-pace");
  expect(quotaPool?.weights).toBeUndefined();
});

test("model service list is the default and routing controls are a secondary workspace", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/model-services.html");
  const list = page.getByRole("region", { name: "模型服务列表", exact: true });
  const routing = page.getByRole("region", {
    name: "外部路由策略",
    exact: true,
  });
  const create = page.getByRole("button", {
    name: "创建模型服务",
    exact: true,
  });
  await expect(list.getByRole("table")).toBeInViewport();
  await expect(routing).toHaveCount(0);
  await page.getByRole("tab", { name: "外部路由策略", exact: true }).click();
  await expect(routing).toBeVisible();
  await expect(list).toHaveCount(0);
  await expect(create).toBeInViewport();
  await page.getByRole("tab", { name: "服务列表", exact: true }).click();
  await expect(list.getByRole("table")).toBeVisible();
});

test("create a cross-provider model service and publish capability metadata", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?empty=1");
  await expect(
    page.getByRole("region", { name: "模型服务列表" }).getByRole("table"),
  ).toContainText("尚未创建模型服务");
  await page.getByRole("button", { name: "创建模型服务" }).click();
  await page
    .getByRole("textbox", { name: "服务名称", exact: true })
    .fill("研发组");
  await page.getByRole("button", { name: "添加模型" }).click();
  await page
    .getByRole("textbox", { name: "显示名称", exact: true })
    .fill("Pro");
  await page.getByRole("combobox", { name: "来源模型", exact: true }).click();
  await page.getByRole("option", { name: "a", exact: true }).click();
  await page.getByRole("button", { name: "添加模型" }).click();
  await page
    .getByRole("textbox", { name: "显示名称", exact: true })
    .fill("Flash");
  await page
    .getByRole("combobox", { name: "Provider 来源", exact: true })
    .fill("Beta");
  await page.getByRole("option", { name: "Beta", exact: true }).click();
  await page.getByRole("combobox", { name: "来源模型", exact: true }).click();
  await page.getByRole("option", { name: "b", exact: true }).click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("96000");
  await page.getByRole("button", { name: "验证并发布" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const row = page.getByRole("row").filter({ hasText: "研发组" });
  await expect(row).toContainText("2 个 Provider");
  await expect(row).toContainText("Pro · Flash");
  const saved = await page.evaluate(
    () => window.modelServiceFixture.catalog.services[0],
  );
  expect(saved.models[1].capabilities.contextWindow).toBe(96000);
  expect(saved.models.map((m) => m.provider.id)).toEqual([
    "source-Alpha",
    "source-Beta",
  ]);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["verify:source-Alpha:a", "verify:source-Beta:b", "save"],
  );
});

test("previews enrollment using only the model's published upstream accounts", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/model-services.html?agent=opencode&upstream-accounts=1&require-account=1",
  );
  await page
    .getByRole("combobox", { name: "OpenCode 模型连接", exact: true })
    .click();
  await page.getByRole("option", { name: "编程服务", exact: true }).click();
  await page.getByRole("button", { name: "验证并生成预览" }).click();
  await expect(
    page.getByRole("button", { name: "写入配置", exact: true }),
  ).toBeEnabled();
  const actions = await page.evaluate(() => window.modelServiceFixture.actions);
  expect(actions.filter((action) => /^(account|verify):/.test(action))).toEqual(
    [
      "account:a8259fd0-66d1-493f-ac6f-12abc5437890",
      "verify:source-Alpha:a",
      "account:b8259fd0-66d1-493f-ac6f-12abc5437890",
      "verify:source-Alpha:a",
      "verify:source-Beta:b",
    ],
  );
});

test("publication preview shows account status and invalidates edited scope", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto(
    "/tests/frontend/model-services.html?require-account=1&upstream-accounts=1",
  );
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  const preview = page.getByRole("region", { name: "发布前路由预览" });
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    [],
  );
  await preview.getByRole("button", { name: "读取预览", exact: true }).click();
  await expect(preview).toContainText("核对时间");
  await expect(preview).toContainText("验证已过期");
  await expect(preview).toContainText("发布时将验证");
  await expect(preview).toContainText("上游限流");
  await expect(preview).toContainText("正在探测额度恢复");
  await expect(preview).toContainText("账号已暂停");
  await expect(preview).toContainText("跨模型回退未开启");

  const entry = preview.getByRole("combobox", { name: "预览入口" });
  await entry.click();
  await page.getByRole("option", { name: "指定模型 · Flash" }).click();
  await expect(preview).toContainText("能力与此入口不匹配");
  await expect(entry).toHaveAttribute("aria-expanded", "false");
  await entry.click();
  await page.getByRole("option", { name: "默认入口 · 未命中规则" }).click();
  await expect(preview).toContainText("跨模型回退未开启");

  await page
    .getByRole("switch", { name: "账号池耗尽后允许跨模型回退" })
    .click();
  await expect(preview).toContainText("请重新读取预览");
  await expect(preview.getByText("账号已暂停")).toHaveCount(0);
  await page.evaluate(() => {
    window.modelServiceFixture.previewQuota = "recovered";
    window.modelServiceFixture.previewRecovery = "probing";
  });
  await preview.getByRole("button", { name: "重新读取预览" }).click();
  await expect(preview).toContainText("允许回退");
  await expect(preview).toContainText("额度恢复探测通过");
  await expect(preview).toContainText("上游限流，账号恢复探测中");
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    [
      "preview:default",
      "preview:specified",
      "preview:default",
      "preview:default",
    ],
  );
});

test("publish conflicts preserve the editor and deleting an attached service stays blocked", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "服务名称", exact: true })
    .fill("编程服务二");
  await page.evaluate(() => {
    window.modelServiceFixture.failSave = true;
  });
  await page.getByRole("button", { name: "验证并发布" }).click();
  await expect(page.getByRole("dialog")).toContainText("模型服务已被修改");
  await expect(
    page.getByRole("textbox", { name: "服务名称", exact: true }),
  ).toHaveValue("编程服务二");
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await page.evaluate(() => {
    window.modelServiceFixture.inUse = true;
  });
  await page
    .getByRole("button", { name: "删除 编程服务", exact: true })
    .click();
  await page.getByRole("button", { name: "删除服务", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("此服务仍被 Agent 接入");
  expect(
    await page.evaluate(
      () => window.modelServiceFixture.catalog.services.length,
    ),
  ).toBe(1);
  await page.evaluate(() => {
    window.modelServiceFixture.inUse = false;
  });
  await page.getByRole("button", { name: "删除服务", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(
    page.getByRole("region", { name: "模型服务列表" }).getByRole("table"),
  ).toContainText("尚未创建模型服务");
});

test("native enrollment blocks running clients, preserves dismissed previews, and restores through a preview", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/model-services.html?agent=opencode&running=1",
  );
  await page
    .getByRole("combobox", { name: "OpenCode 模型连接", exact: true })
    .click();
  await page.getByRole("option", { name: "编程服务", exact: true }).click();
  await page.getByRole("button", { name: "验证并生成预览" }).click();
  const apply = page.getByRole("button", { name: "写入配置", exact: true });
  await expect(apply).toBeEnabled();
  await apply.click();
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText(
    "配置未写入",
  );
  expect(
    await page.evaluate(() => window.modelServiceFixture.config.revision),
  ).toBe("original");
  await page.keyboard.press("Escape");
  const history = page.getByRole("region", { name: "配置切换与备份记录" });
  await expect(history).toContainText("待应用");
  await page.evaluate(() => {
    window.modelServiceFixture.processes = 0;
    window.dispatchEvent(new Event("provider-status-changed"));
  });
  const applyPreview = async () => {
    await history
      .getByRole("button", { name: "重启 Agent 并生效", exact: true })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "已退出，应用配置", exact: true })
      .click();
  };
  await applyPreview();
  await expect(page.getByText("默认：Pro", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("status", { name: "OpenCode 当前连接", exact: true }),
  ).toHaveText("模型服务 · 编程服务");
  await page
    .getByRole("button", { name: "预览恢复原配置", exact: true })
    .click();
  await expect(history).toContainText("恢复服务接入前配置");
  expect(
    await page.evaluate(() => window.modelServiceFixture.config.revision),
  ).toBe("installed");
  await applyPreview();
  await expect(
    page.getByText("尚未接入模型服务", { exact: true }),
  ).toBeVisible();
});

test("small window keeps model fields and publish actions reachable and recovers from read errors", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("combobox", { name: "服务默认模型", exact: true })
    .click();
  await page.getByRole("option", { name: "Flash", exact: true }).press("Enter");
  await page.getByRole("button", { name: "验证并发布" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(
      () => window.modelServiceFixture.catalog.services[0].defaultModelId,
    ),
  ).toBe("model-flash");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.evaluate(() => {
    window.modelServiceFixture.failRead = true;
  });
  await page.getByRole("button", { name: "重新读取", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("服务目录读取失败");
  await expect(
    page.getByRole("button", { name: "创建模型服务" }),
  ).toBeDisabled();
  await page.evaluate(() => {
    window.modelServiceFixture.failRead = false;
  });
  await page.getByRole("button", { name: "重新读取", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "创建模型服务" }),
  ).toBeEnabled();
});

test("unreadable installed configurations keep their unknown connection state", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page.evaluate(() => {
    const service = window.modelServiceFixture.catalog.services[0];
    window.modelServiceFixture.catalog.attachments.push({
      bindingId: "unknown-binding",
      serviceId: service.id,
      installed: service,
      agentId: "claude",
      localEndpoint: "http://127.0.0.1:1/gateway/unknown",
      attached: false,
      referenced: false,
      configurationUnknown: true,
      directorySyncRequired: false,
    });
  });
  await page.getByRole("button", { name: "重新读取", exact: true }).click();
  await expect(
    page.getByRole("row").filter({ hasText: "编程服务" }),
  ).toContainText("有配置未知");
});

test("routing priority and per-model limits publish without rewriting the model catalog", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page.getByRole("switch", { name: "启用优先级与自动回退" }).click();
  await page
    .getByRole("button", { name: "提高 Flash 优先级", exact: true })
    .click();
  await expect(
    page.getByRole("combobox", { name: "服务默认模型", exact: true }),
  ).toContainText("Flash");
  await page.getByRole("spinbutton", { name: "最多尝试候选" }).fill("2");
  await page.getByRole("spinbutton", { name: "来源模型最大并发" }).fill("3");
  await page.getByRole("button", { name: "验证并发布" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const saved = await page.evaluate(
    () => window.modelServiceFixture.catalog.services[0],
  );
  expect(saved.defaultModelId).toBe("model-flash");
  expect(saved.models.map((m) => m.id)).toEqual(["model-pro", "model-flash"]);
  expect(saved.routingPolicy?.order).toEqual(["model-flash", "model-pro"]);
  expect(saved.routingPolicy?.enabled).toBe(true);
  expect(saved.routingPolicy?.maxAttempts).toBe(2);
  expect(saved.routingPolicy?.limits[0].maxConcurrent).toBe(3);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["save"],
  );
});

test("cooldown state remains distinct from unavailable status metadata", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page.evaluate(() => {
    window.modelServiceFixture.catalog.routingHealth = [
      {
        serviceId: "fixture-service",
        modelId: "model-pro",
        status: {
          state: "cooling",
          untilMs: Date.now() + 10000,
          reason: "upstream_rate_limit",
        },
      },
    ];
  });
  await page.getByRole("button", { name: "重新读取", exact: true }).click();
  await expect(
    page.getByRole("row").filter({ hasText: "编程服务" }),
  ).toContainText("1 个模型来源冷却中");
  for (const [state, label] of [
    ["waitingForProbe", "1 个模型来源待恢复探测"],
    ["probing", "1 个模型来源恢复探测中"],
  ] as const) {
    await page.evaluate((phase) => {
      const [primary] = window.modelServiceFixture.catalog.routingHealth!;
      primary.status.state = phase;
      primary.status.untilMs = Date.now() - 1000;
    }, state);
    await page.getByRole("button", { name: "重新读取", exact: true }).click();
    const row = page.getByRole("row").filter({ hasText: "编程服务" });
    await expect(row).toContainText(label);
    await expect(row).not.toContainText("来源无冷却记录");
  }
});

test("dismissed service previews resume without preparing or verifying a second time", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?agent=opencode");
  await page
    .getByRole("combobox", { name: "OpenCode 模型连接", exact: true })
    .click();
  await page.getByRole("option", { name: "编程服务", exact: true }).click();
  await page
    .getByRole("button", { name: "验证并生成预览", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "写入配置", exact: true }),
  ).toBeEnabled();
  const actions = await page.evaluate(() => [
    ...window.modelServiceFixture.actions,
  ]);
  await page.keyboard.press("Escape");
  await expect(page.getByText("有未完成的切换", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "OpenCode 模型连接", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "继续处理", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "写入配置", exact: true }),
  ).toBeEnabled();
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    actions,
  );
  await page.getByRole("button", { name: "写入配置", exact: true }).click();
  await page.getByRole("button", { name: "完成", exact: true }).click();
  await expect(page.getByText("默认：Pro", { exact: true })).toBeVisible();
  await expect(page.getByText("有未完成的切换", { exact: true })).toHaveCount(
    0,
  );
  expect(
    await page.evaluate(
      () =>
        window.modelServiceFixture.actions.filter(
          (a) => a === "prepare-service",
        ).length,
    ),
  ).toBe(1);
});

test("existing previews can be cancelled without changing the native config and release the service picker", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/model-services.html?agent=opencode&pending=1",
  );
  await expect(page.getByText("有未完成的切换", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "OpenCode 模型连接", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "撤销待应用切换", exact: true })
    .click();
  await expect(
    page.getByRole("combobox", { name: "OpenCode 模型连接", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(() => window.modelServiceFixture.config.revision),
  ).toBe("original");
  expect(
    await page.evaluate(() => window.modelServiceFixture.transactions[0].state),
  ).toBe("cancelled");
});

test("a legacy pending transaction still exposes cancellation instead of trapping the picker", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/model-services.html?agent=opencode&pending=1&legacy=1",
  );
  await expect(page.getByText("有未完成的切换", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "继续处理", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "撤销待应用切换", exact: true })
    .click();
  await expect(
    page.getByRole("combobox", { name: "OpenCode 模型连接", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(() => window.modelServiceFixture.config.revision),
  ).toBe("original");
});

test("lost publish reply stays uncertain, locks the draft and queries the original receipt without another verification", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.evaluate(() => {
    window.modelServiceFixture.loseCommitReply = true;
    window.modelServiceFixture.failOperationRead = true;
  });
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("原操作结果尚未确认");
  await expect(
    dialog.getByRole("button", { name: "验证并发布", exact: true }),
  ).toBeDisabled();
  await expect(
    dialog.getByRole("spinbutton", { name: "上下文窗口（Token）" }),
  ).toBeDisabled();
  expect(await dialog.getByText("未发布", { exact: true }).count()).toBe(0);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["verify:source-Alpha:a", "save"],
  );
  const stored = await page.evaluate(
    () =>
      Object.entries(localStorage).find(([key]) =>
        key.startsWith("local-agent.model-service-operation.v1."),
      )?.[1],
  );
  expect(stored).toBeTruthy();
  expect(stored).not.toContain("http://");
  expect(stored).not.toContain("credentialRef");
  await page.evaluate(() => {
    window.modelServiceFixture.failOperationRead = false;
  });
  await dialog
    .getByRole("button", { name: "查询原操作结果", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["verify:source-Alpha:a", "save"],
  );
});

test("reload retains the frozen request, performs no write on mount and retries only the same operation ID", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?write-recovery=1");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.evaluate(() => {
    window.modelServiceFixture.dropBeforeCommit = true;
    window.modelServiceFixture.failOperationRead = true;
  });
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("原操作结果尚未确认");
  const id = await page.evaluate(
    () => window.modelServiceFixture.operationAttempts[0],
  );
  await page.reload();
  const recovery = page.getByRole("region", { name: "模型服务操作核查" });
  await expect(recovery).toContainText(id);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    [],
  );
  expect(
    await page.evaluate(() => window.modelServiceFixture.operationAttempts),
  ).toEqual([]);
  await recovery
    .getByRole("button", { name: "核查已保存的操作", exact: true })
    .click();
  await expect(recovery).toContainText("原操作结果尚未确认");
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    [],
  );
  await recovery
    .getByRole("button", { name: "重试已保存的原操作", exact: true })
    .click();
  await expect(recovery).toContainText("原发布已提交");
  expect(
    await page.evaluate(() => window.modelServiceFixture.operationAttempts),
  ).toEqual([id]);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["save"],
  );
  expect(
    await page.evaluate(
      () =>
        window.modelServiceFixture.catalog.services[0].models[0].capabilities
          .contextWindow,
    ),
  ).toBe(64000);
});

test("a lost deletion reply is recovered from its receipt and never deletes again", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?write-recovery=1");
  await page.evaluate(() => {
    window.modelServiceFixture.loseCommitReply = true;
    window.modelServiceFixture.failOperationRead = true;
  });
  await page
    .getByRole("button", { name: "删除 编程服务", exact: true })
    .click();
  await page.getByRole("button", { name: "删除服务", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("原操作结果尚未确认");
  await page.reload();
  const recovery = page.getByRole("region", { name: "模型服务操作核查" });
  await recovery
    .getByRole("button", { name: "核查已保存的操作", exact: true })
    .click();
  await expect(recovery).toContainText("原删除已提交");
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    [],
  );
  expect(
    await page.evaluate(() => window.modelServiceFixture.catalog.services),
  ).toEqual([]);
});

test("an old backend is detected before model verification or a legacy write", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?legacy-write=1");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText(
    "当前后台不支持模型服务结果核查",
  );
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    [],
  );
  expect(
    await page.evaluate(() => window.modelServiceFixture.operationAttempts),
  ).toEqual([]);
});

test("a pending operation added during verification prevents a second submission", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.evaluate(() => {
    window.modelServiceFixture.holdVerification = true;
  });
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.modelServiceFixture.actions.length))
    .toBe(1);
  const pending = await page.evaluate(() => {
    const request = {
      storeId: window.modelServiceFixture.catalog.writeProtocol!.storeId,
      operationId: crypto.randomUUID(),
      action: {
        kind: "delete",
        serviceId: "fixture-service",
        expectedRevision: 1,
      },
    };
    const key = "local-agent.model-service-operation.v1." + request.operationId;
    const raw = JSON.stringify(request);
    localStorage.setItem(key, raw);
    window.modelServiceFixture.releaseVerification();
    return { key, raw };
  });
  await expect(page.getByRole("dialog")).toContainText("此服务已有待核查操作");
  expect(
    await page.evaluate(() => window.modelServiceFixture.operationAttempts),
  ).toEqual([]);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), pending.key),
  ).toBe(pending.raw);
});

test("publishing requires the original request to survive local storage readback", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.evaluate(() => {
    const save = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (!key.startsWith("local-agent.model-service-operation.v1."))
        save.call(this, key, value);
    };
  });
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("原请求未能完整保存");
  expect(
    await page.evaluate(() => window.modelServiceFixture.operationAttempts),
  ).toEqual([]);
});

test("confirming an original receipt preserves recovery metadata changed during the request", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.evaluate(() => {
    window.modelServiceFixture.loseCommitReply = true;
    window.modelServiceFixture.failOperationRead = true;
  });
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("原操作结果尚未确认");
  const changed = await page.evaluate(() => {
    const [key, raw] = Object.entries(localStorage).find(([key]) =>
      key.startsWith("local-agent.model-service-operation.v1."),
    )!;
    const request = JSON.parse(raw);
    request.action.draft.name = "另一个待核查请求";
    const replacement = JSON.stringify(request);
    localStorage.setItem(key, replacement);
    window.modelServiceFixture.failOperationRead = false;
    return { key, raw: replacement };
  });
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "查询原操作结果", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), changed.key),
  ).toBe(changed.raw);
});

test("unmounting while verifying does not verify the next model or submit", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.getByRole("button", { name: "Flash", exact: true }).click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("128000");
  await page.evaluate(() => {
    window.modelServiceFixture.holdVerification = true;
  });
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.modelServiceFixture.actions.length))
    .toBe(1);
  await page.evaluate(() => {
    window.modelServiceFixture.unmountView();
    window.modelServiceFixture.releaseVerification();
  });
  await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 40)));
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["verify:source-Alpha:a"],
  );
  expect(
    await page.evaluate(() => window.modelServiceFixture.operationAttempts),
  ).toEqual([]);
});

test("pending requests cannot be replayed into a different database and retain their original scope", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.evaluate(() => {
    window.modelServiceFixture.dropBeforeCommit = true;
    window.modelServiceFixture.failOperationRead = true;
  });
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("原操作结果尚未确认");
  await page.evaluate(() => {
    window.modelServiceFixture.failOperationRead = false;
    window.modelServiceFixture.catalog.writeProtocol!.storeId =
      "00000000-0000-4000-8000-000000000002";
  });
  await dialog
    .getByRole("button", { name: "按原编号重试", exact: true })
    .click();
  await expect(dialog).toContainText("另一个本机数据库");
  expect(
    await page.evaluate(
      () => window.modelServiceFixture.operationAttempts.length,
    ),
  ).toBe(1);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["verify:source-Alpha:a"],
  );
  await page.evaluate(() => {
    window.modelServiceFixture.catalog.writeProtocol!.storeId =
      "00000000-0000-4000-8000-000000000001";
  });
  await dialog
    .getByRole("button", { name: "按原编号重试", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  const attempts = await page.evaluate(
    () => window.modelServiceFixture.operationAttempts,
  );
  expect(attempts).toHaveLength(2);
  expect(attempts[0]).toBe(attempts[1]);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["verify:source-Alpha:a", "save"],
  );
});

for (const action of ["publish", "delete"] as const) {
  test(`${action} retains the dialog origin when the backend changes before its first submission`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/model-services.html");
    await page
      .getByRole("button", {
        name: action === "publish" ? "编辑 编程服务" : "删除 编程服务",
        exact: true,
      })
      .click();
    if (action === "publish") {
      await page
        .getByRole("textbox", { name: "服务名称", exact: true })
        .fill("A库草稿");
      await page
        .getByRole("spinbutton", { name: "上下文窗口（Token）" })
        .fill("64000");
    }
    await page.evaluate(() => {
      const catalog = window.modelServiceFixture.catalog;
      catalog.writeProtocol!.storeId = "00000000-0000-4000-8000-000000000002";
      catalog.services[0].name = "B库同号服务";
      window.dispatchEvent(new Event("model-services-changed"));
    });
    await expect(
      page.getByRole("region", { name: "模型服务列表", includeHidden: true }),
    ).toContainText("B库同号服务");
    const dialog = page.getByRole("dialog");
    await dialog
      .getByRole("button", {
        name: action === "publish" ? "验证并发布" : "删除服务",
        exact: true,
      })
      .click();
    await expect(dialog).toContainText("另一个本机数据库");
    if (action === "publish")
      await expect(
        dialog.getByRole("textbox", { name: "服务名称", exact: true }),
      ).toHaveValue("A库草稿");
    else await expect(dialog).toContainText("删除“编程服务”");
    expect(
      await page.evaluate(() => window.modelServiceFixture.actions),
    ).toEqual([]);
    expect(
      await page.evaluate(() => window.modelServiceFixture.operationAttempts),
    ).toEqual([]);
    expect(
      await page.evaluate(() =>
        Object.keys(localStorage).filter((key) =>
          key.startsWith("local-agent.model-service-operation.v1."),
        ),
      ),
    ).toEqual([]);
    await page.evaluate(() => {
      const catalog = window.modelServiceFixture.catalog;
      catalog.writeProtocol!.storeId = "00000000-0000-4000-8000-000000000001";
      catalog.services[0].name = "编程服务";
      window.dispatchEvent(new Event("model-services-changed"));
    });
    await expect(
      page.getByRole("region", { name: "模型服务列表", includeHidden: true }),
    ).toContainText("编程服务");
    await dialog
      .getByRole("button", {
        name: action === "publish" ? "验证并发布" : "删除服务",
        exact: true,
      })
      .click();
    await expect(dialog).toHaveCount(0);
    expect(
      await page.evaluate(() => window.modelServiceFixture.actions),
    ).toEqual(
      action === "publish" ? ["verify:source-Alpha:a", "save"] : ["delete"],
    );
  });
}

test("a dialog opened without a database identity cannot adopt one after a protocol upgrade", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?legacy-write=1");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "服务名称", exact: true })
    .fill("未确认来源的草稿");
  await page.evaluate(() => {
    const catalog = window.modelServiceFixture.catalog;
    catalog.writeProtocol = {
      version: 1,
      storeId: "00000000-0000-4000-8000-000000000001",
      receiptLimit: 256,
    };
    catalog.services[0].name = "新后台目录";
    window.dispatchEvent(new Event("model-services-changed"));
  });
  await expect(
    page.getByRole("region", { name: "模型服务列表", includeHidden: true }),
  ).toContainText("新后台目录");
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(dialog).toContainText("无法确认草稿所属后台");
  await expect(
    dialog.getByRole("textbox", { name: "服务名称", exact: true }),
  ).toHaveValue("未确认来源的草稿");
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    [],
  );
  expect(
    await page.evaluate(() => window.modelServiceFixture.operationAttempts),
  ).toEqual([]);
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await page
    .getByRole("button", { name: "编辑 新后台目录", exact: true })
    .click();
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    ["save"],
  );
});

test("receipt parsing rejects identity, scope, revision, null and malformed nested capabilities", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html");
  const results = await page.evaluate(() => {
    const fixture = window.modelServiceFixture,
      s = fixture.catalog.services[0];
    const request = {
      storeId: fixture.catalog.writeProtocol!.storeId,
      operationId: crypto.randomUUID(),
      action: {
        kind: "save" as const,
        draft: {
          id: s.id,
          revision: s.revision - 1,
          name: s.name,
          defaultModelId: s.defaultModelId,
          routingPolicy: s.routingPolicy,
          models: s.models.map((m) => ({
            id: m.id,
            name: m.name,
            providerId: m.provider.id,
            model: m.provider.model,
            capabilities: m.capabilities,
          })),
        },
      },
    };
    const receipt = {
      apiVersion: 1 as const,
      storeId: request.storeId,
      operationId: request.operationId,
      action: "save" as const,
      serviceId: s.id,
      committedRevision: s.revision,
      service: s,
    };
    fixture.parseReceipt(receipt, request);
    const variants: unknown[] = [
      { ...receipt, operationId: crypto.randomUUID() },
      { ...receipt, storeId: crypto.randomUUID() },
      { ...receipt, committedRevision: "1" },
      { ...receipt, committedRevision: Number.MAX_SAFE_INTEGER + 1 },
      { ...receipt, apiVersion: 2 },
      { ...receipt, service: null },
      { ...receipt, service: { ...s, models: [] } },
      {
        ...receipt,
        service: {
          ...s,
          models: s.models.map((m) => ({
            ...m,
            capabilities: { ...m.capabilities, tools: "yes" },
          })),
        },
      },
    ];
    return variants.map((value) => {
      try {
        fixture.parseReceipt(value, request);
        return false;
      } catch (error) {
        const e = error as { code: string; outcomeUnknown: boolean };
        return e.code === "transport_invalid_response" && e.outcomeUnknown;
      }
    });
  });
  expect(results).toEqual(Array(8).fill(true));
});

test("unreadable recovery metadata remains visible and never triggers a verification or write", async ({
  page,
}) => {
  await page.addInitScript(() =>
    localStorage.setItem(
      "local-agent.model-service-operation.v1.00000000-0000-4000-8000-000000000009",
      "{invalid-fixture",
    ),
  );
  await page.goto("/tests/frontend/model-services.html");
  await expect(
    page.getByRole("region", { name: "模型服务操作核查" }),
  ).toContainText("操作恢复记录不可读取");
  await page
    .getByRole("button", { name: "编辑 编程服务", exact: true })
    .click();
  await page
    .getByRole("spinbutton", { name: "上下文窗口（Token）" })
    .fill("64000");
  await page.getByRole("button", { name: "验证并发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("恢复记录不可读取");
  expect(await page.evaluate(() => window.modelServiceFixture.actions)).toEqual(
    [],
  );
  expect(
    await page.evaluate(() => window.modelServiceFixture.operationAttempts),
  ).toEqual([]);
});
