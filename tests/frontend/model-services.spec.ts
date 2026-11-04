import { expect, test } from "@playwright/test";

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
    .getByRole("combobox", { name: "OpenCode 模型服务", exact: true })
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
  await page.getByText("配置备份与恢复（1）", { exact: true }).click();
  await expect(
    page.getByRole("region", { name: "模型服务配置备份" }).getByRole("table"),
  ).toContainText("待应用");
  await page.evaluate(() => {
    window.modelServiceFixture.processes = 0;
    window.dispatchEvent(new Event("provider-status-changed"));
  });
  await page.getByRole("button", { name: "应用预览", exact: true }).click();
  await expect(page.getByText("默认：Pro", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("combobox", { name: "OpenCode 模型服务", exact: true }),
  ).toContainText("编程服务");
  await page
    .getByRole("button", { name: "预览恢复原配置", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "模型服务配置备份" }).getByRole("table"),
  ).toContainText("恢复服务接入前配置");
  expect(
    await page.evaluate(() => window.modelServiceFixture.config.revision),
  ).toBe("installed");
  await page.getByRole("button", { name: "应用预览", exact: true }).click();
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
});

test("dismissed service previews resume without preparing or verifying a second time", async ({
  page,
}) => {
  await page.goto("/tests/frontend/model-services.html?agent=opencode");
  await page
    .getByRole("combobox", { name: "OpenCode 模型服务", exact: true })
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
    page.getByRole("combobox", { name: "OpenCode 模型服务", exact: true }),
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
    page.getByRole("combobox", { name: "OpenCode 模型服务", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "撤销待应用切换", exact: true })
    .click();
  await expect(
    page.getByRole("combobox", { name: "OpenCode 模型服务", exact: true }),
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
    page.getByRole("combobox", { name: "OpenCode 模型服务", exact: true }),
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
