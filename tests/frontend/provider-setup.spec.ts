import { expect, test, type Page } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";

async function open(page: Page, mode = "") {
  await page.goto(`/tests/frontend/provider-auto-connect.html?mode=${mode}`);
  await page.getByRole("combobox", { name: "供应商", exact: true }).click();
  await page.getByRole("option", { name: "DeepSeek", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "服务地址", exact: true }),
  ).toHaveValue("https://api.deepseek.com");
}
const keyInput = (page: Page) =>
  page.getByRole("textbox", { name: "API Key", exact: true });
const getModels = (page: Page) =>
  page.getByRole("button", { name: "获取模型", exact: true });
const confirmation = (page: Page) =>
  page.getByRole("dialog", { name: "确认添加新的 API Key", exact: true });
async function chooseModel(page: Page) {
  const picker = page.getByRole("combobox", { name: "默认模型", exact: true });
  await picker.fill("fixture-chat");
  await page
    .getByRole("option", { name: "Fixture Chat · fixture-chat", exact: true })
    .click();
}

test("supplier then API Key then catalog then selected model saves without inference", async ({
  page,
}) => {
  await open(page);
  await expect(
    page.getByRole("combobox", { name: "接口协议", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "保存连接", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByText("凭据恢复空间尚未核查", { exact: true }),
  ).toHaveCount(0);
  await keyInput(page).fill("disposable-setup-key");
  await getModels(page).click();
  await expect(keyInput(page)).toHaveValue("");
  await chooseModel(page);
  const beforeSave = await page.evaluate(() => window.autoConnectFixture);
  expect(beforeSave.modelRequests).toHaveLength(1);
  expect(beforeSave.modelRequests[0]).toMatchObject({
    vendor: "deepseek",
    protocol: "chat-completions",
    endpoint: "https://api.deepseek.com",
  });
  expect(beforeSave.modelRequests[0].credentialRef).toBeTruthy();
  expect(beforeSave.credentials).toHaveLength(1);
  expect(beforeSave.manualSaves).toHaveLength(0);
  expect(beforeSave.connections).toHaveLength(0);
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const saved = await page.evaluate(
    () => window.autoConnectFixture.manualSaves[0],
  );
  expect(saved).toMatchObject({
    name: "DeepSeek",
    vendor: "deepseek",
    model: "fixture-chat",
    models: ["fixture-chat", "fixture-reasoner"],
  });
  expect(saved.modelCatalog?.credentialRef).toBe(saved.credentialRef);
  expect(JSON.stringify(saved)).not.toContain("disposable-setup-key");
  expect(
    await page.evaluate(() => window.autoConnectFixture.connections.length),
  ).toBe(0);
});

test("missing Key does not request a remote catalog or initialize recovery", async ({
  page,
}) => {
  await open(page);
  await getModels(page).click();
  await expect(page.getByText(/请先填写此供应商的 API Key/)).toBeVisible();
  expect(
    await page.evaluate(() => window.autoConnectFixture.modelRequests),
  ).toHaveLength(0);
  expect(
    await page.evaluate(() =>
      window.providerOperationsFixture.commands.filter(
        (row) => row.name === "initialize_provider_credential_namespace",
      ),
    ),
  ).toHaveLength(0);
});

test("replacing the Key after fetching does not stamp the old catalog as current", async ({
  page,
}) => {
  await open(page);
  await keyInput(page).fill("disposable-first-key");
  await getModels(page).click();
  await chooseModel(page);
  await keyInput(page).fill("disposable-replacement-key");
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const fixture = await page.evaluate(() => window.autoConnectFixture);
  expect(fixture.credentials).toHaveLength(2);
  expect(fixture.manualSaves[0].modelCatalog).toBeNull();
  expect(fixture.manualSaves[0].models).toEqual([
    "fixture-chat",
    "fixture-reasoner",
  ]);
  expect(fixture.connections).toHaveLength(0);
});

test("a directory failure reuses the saved Key; manual fallback remains available", async ({
  page,
}) => {
  await open(page, "models-error");
  await keyInput(page).fill("disposable-setup-key");
  await getModels(page).click();
  await expect(page.getByText("未能获取模型", { exact: true })).toBeVisible();
  await getModels(page).click();
  await expect
    .poll(() =>
      page.evaluate(() => window.autoConnectFixture.modelRequests.length),
    )
    .toBe(2);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(1);
  await page
    .getByRole("button", { name: "手动输入模型 ID", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "默认模型", exact: true })
    .fill("manual-model");
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.manualSaves[0].model),
  ).toBe("manual-model");
});

test("old recovery is an optional entry and never initializes or replays old requests", async ({
  page,
}) => {
  const oldKey =
    "local-agent.credential-operation.v2.c9e027fe-4536-4d08-8a55-74e6cbaf6313.cf45b4a0-4dac-4bd5-b276-3181d3e1514e";
  const oldRaw = JSON.stringify({
    version: 2,
    storeId: "c9e027fe-4536-4d08-8a55-74e6cbaf6313",
    operationId: "cf45b4a0-4dac-4bd5-b276-3181d3e1514e",
    label: "Old pending",
    cancelledPending: false,
  });
  await page.addInitScript(({ key, raw }) => localStorage.setItem(key, raw), {
    key: oldKey,
    raw: oldRaw,
  });
  await open(page, "credential-unverified");
  await expect(
    page.getByText("凭据恢复空间尚未核查", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByText("Old pending", { exact: true })).toHaveCount(0);
  await keyInput(page).fill("disposable-setup-key");
  await page
    .getByRole("button", { name: "API Key 保存与恢复", exact: true })
    .click();
  await expect(
    page.getByText("凭据恢复空间尚未核查", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Old pending", { exact: true })).toBeVisible();
  await expect(keyInput(page)).toHaveValue("disposable-setup-key");
  expect(
    await page.evaluate(() =>
      window.providerOperationsFixture.commands.filter(
        (row) => row.name === "initialize_provider_credential_namespace",
      ),
    ),
  ).toHaveLength(0);
  await page
    .getByRole("button", { name: "收起 API Key 恢复选项", exact: true })
    .click();
  await getModels(page).click();
  await chooseModel(page);
  await expect(confirmation(page)).toHaveCount(0);
  await expect(page.getByText("Old pending", { exact: true })).toHaveCount(0);
  expect(await page.evaluate((key) => localStorage.getItem(key), oldKey)).toBe(
    oldRaw,
  );
  expect(
    await page.evaluate(() => ({
      checks: window.autoConnectFixture.credentialChecks,
      cancels: window.autoConnectFixture.credentialCancels,
    })),
  ).toEqual({ checks: [], cancels: [] });
  expect(
    await page.evaluate(
      () =>
        Object.values(window.providerOperationsFixture.snapshot().stores)[0]
          .legacyState,
    ),
  ).toBe("unverified");
});

for (const mode of [
  "credential-unverified",
  "credential-unverified-init-lost",
]) {
  test(`${mode}: new Key fetches and saves directly without a confirmation or inference`, async ({
    page,
  }) => {
    await open(page, mode);
    await keyInput(page).fill("disposable-setup-key");
    await getModels(page).click();
    await expect(confirmation(page)).toHaveCount(0);
    await chooseModel(page);
    const fixture = await page.evaluate(() => window.autoConnectFixture);
    expect(fixture.credentials).toHaveLength(1);
    expect(fixture.modelRequests).toHaveLength(1);
    expect(fixture.connections).toHaveLength(0);
    expect(fixture.manualSaves).toHaveLength(0);
    expect(
      await page.evaluate(
        () =>
          Object.values(window.providerOperationsFixture.snapshot().stores)[0]
            .legacyState,
      ),
    ).toBe("unverified");
    const initializations = await page.evaluate(() =>
      window.providerOperationsFixture.commands.filter(
        (row) => row.name === "initialize_provider_credential_namespace",
      ),
    );
    expect(initializations).toHaveLength(1);
    expect(initializations[0].args).toMatchObject({
      request: { choice: "new" },
    });
    await page
      .getByRole("button", { name: "API Key 保存与恢复", exact: true })
      .click();
    await expect(
      page.getByText("旧凭据恢复材料尚未核查", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "核查原恢复材料", exact: true }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "收起 API Key 恢复选项", exact: true })
      .click();
    await page.getByRole("button", { name: "保存连接", exact: true }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(
      await page.evaluate(() => window.autoConnectFixture.credentials),
    ).toHaveLength(1);
  });
}

for (const source of ["namespace", "database"] as const) {
  test(`a changed ${source} during preparation cannot send the new Key`, async ({
    page,
  }) => {
    await open(page, "credential-unverified-init-hold");
    await keyInput(page).fill("disposable-setup-key");
    await getModels(page).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.providerOperationsFixture.commands.filter(
              (row) => row.name === "initialize_provider_credential_namespace",
            ).length,
        ),
      )
      .toBe(1);
    await page.evaluate((source) => {
      if (source === "namespace")
        window.providerOperationsFixture.switchNamespace(
          "f776744f-9a29-4498-9588-fbf3bfb07444",
        );
      else {
        const id = "6087597b-0625-42ea-8fd0-e51f6ad16d01";
        window.providerOperationsFixture.switchStore(id);
        window.autoConnectFixture.sourceChange?.(id);
      }
      window.providerOperationsFixture.release();
    }, source);
    await expect(keyInput(page)).toHaveValue("disposable-setup-key");
    await expect(confirmation(page)).toHaveCount(0);
    expect(
      await page.evaluate(() => window.autoConnectFixture.credentials),
    ).toHaveLength(0);
    expect(
      await page.evaluate(() => window.autoConnectFixture.modelRequests),
    ).toHaveLength(0);
  });
}

test("preparation failure keeps the new Key and retry only creates it once", async ({
  page,
}) => {
  await open(page, "credential-unverified-init-error");
  await keyInput(page).fill("disposable-setup-key");
  await getModels(page).click();
  await expect(page.getByRole("status")).toContainText(
    "新 Key 保存准备暂不可用",
  );
  await expect(keyInput(page)).toHaveValue("disposable-setup-key");
  await expect(confirmation(page)).toHaveCount(0);
  await expect(
    page.getByText("凭据恢复空间尚未核查", { exact: true }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(0);
  await page.evaluate(() => {
    window.providerOperationsFixture.failInitialization = false;
  });
  await getModels(page).click();
  await chooseModel(page);
  expect(
    await page.evaluate(() => window.autoConnectFixture.credentials),
  ).toHaveLength(1);
  expect(
    await page.evaluate(() => window.autoConnectFixture.modelRequests),
  ).toHaveLength(1);
});

test("manual model save prepares a new Key without a confirmation or inference", async ({
  page,
}) => {
  await open(page, "credential-unverified");
  await keyInput(page).fill("disposable-setup-key");
  await page
    .getByRole("button", { name: "手动输入模型 ID", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "默认模型", exact: true })
    .fill("manual-model");
  await page.getByRole("button", { name: "保存连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const fixture = await page.evaluate(() => window.autoConnectFixture);
  expect(fixture.credentials).toHaveLength(1);
  expect(fixture.manualSaves[0].model).toBe("manual-model");
  expect(fixture.connections).toHaveLength(0);
  expect(fixture.modelRequests).toHaveLength(0);
});

test("a catalog arriving after recovery source changes is discarded", async ({
  page,
}) => {
  await open(page, "models-hold");
  await keyInput(page).fill("disposable-setup-key");
  await getModels(page).click();
  await expect
    .poll(() =>
      page.evaluate(() => window.autoConnectFixture.modelRequests.length),
    )
    .toBe(1);
  await page.evaluate(() => {
    window.providerOperationsFixture.switchNamespace(
      "f776744f-9a29-4498-9588-fbf3bfb07444",
    );
    window.autoConnectFixture.release?.();
  });
  await expect(getModels(page)).toBeEnabled();
  await page
    .getByRole("combobox", { name: "默认模型", exact: true })
    .fill("fixture-chat");
  await expect(page.getByRole("option", { name: /fixture-chat/ })).toHaveCount(
    0,
  );
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("button", { name: "保存连接", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(() => window.autoConnectFixture.manualSaves),
  ).toHaveLength(0);
});

test("empty catalog is explicit and the narrow dark form keeps keyboard actions reachable", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await open(page, "models-empty");
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await keyInput(page).fill("disposable-setup-key");
  await getModels(page).click();
  await expect(page.getByText(/服务商返回空目录/)).toBeVisible();
  await expect(
    page.getByRole("button", { name: "保存连接", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await getModels(page).focus();
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "手动输入模型 ID", exact: true }),
  ).toBeFocused();
  await expect(
    page.getByRole("button", { name: "手动输入模型 ID", exact: true }),
  ).toBeInViewport();
});
