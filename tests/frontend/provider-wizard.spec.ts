import { expect, test } from "@playwright/test";
import type {} from "./provider-auto-connect-harness";

test("guided provider onboarding probes before saving and offers a directory and manual models together", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-auto-connect.html?guided");
  const wizard = page.getByRole("region", { name: "添加模型供应商" });
  await expect(wizard.getByRole("button", { name: "下一步" })).toBeDisabled();
  await wizard.getByRole("combobox", { name: "供应商", exact: true }).click();
  await page.getByRole("option", { name: "DeepSeek", exact: true }).click();
  await expect(
    wizard.getByRole("combobox", { name: "接口协议" }),
  ).toBeVisible();
  await wizard.getByRole("button", { name: "下一步" }).click();
  await wizard
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("fixture-key");
  await wizard
    .getByRole("textbox", { name: "探活模型 ID" })
    .fill("fixture-chat");
  await wizard.getByRole("button", { name: "验证连接", exact: true }).click();
  await expect(wizard.getByText("短请求验证通过")).toBeVisible();
  expect(
    await page.evaluate(() => ({
      models: window.autoConnectFixture.draftProbes.map((item) => item.model),
      saved: window.autoConnectFixture.saved,
      reads: window.autoConnectFixture.modelRequests.length,
    })),
  ).toEqual({ models: ["fixture-chat"], saved: 0, reads: 0 });

  await wizard.getByRole("button", { name: "继续并获取模型" }).click();
  await expect(wizard.getByRole("button", { name: "刷新目录" })).toBeVisible();
  await wizard.getByRole("combobox", { name: "默认模型" }).click();
  await page.getByRole("option", { name: /Fixture Reasoner/ }).click();
  await wizard.getByRole("checkbox", { name: "加入 fixture-chat" }).uncheck();
  await wizard
    .getByRole("textbox", { name: "补充模型目录" })
    .fill("extra-model, another-model");
  await expect(
    wizard.getByRole("checkbox", { name: "同时生成同名默认模型服务" }),
  ).toBeChecked();
  await wizard.getByRole("button", { name: "保存并前往绑定 Agent" }).click();
  await expect(wizard).toHaveCount(0);
  const result = await page.evaluate(() => ({
    provider: window.autoConnectFixture.manualSaves[0],
    createService: window.autoConnectFixture.makeDefaultService,
    credentialWrites: window.autoConnectFixture.credentials.length,
  }));
  expect(result.provider.model).toBe("fixture-reasoner");
  expect(result.provider.models).toEqual(
    expect.arrayContaining([
      "fixture-reasoner",
      "extra-model",
      "another-model",
    ]),
  );
  expect(result.provider.models).not.toContain("fixture-chat");
  expect(result.createService).toBe(true);
  expect(result.credentialWrites).toBe(1);
});

test("empty directory offers a manual default and a save without publishing a model service", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-auto-connect.html?guided&mode=models-empty",
  );
  const wizard = page.getByRole("region", { name: "添加模型供应商" });
  await wizard.getByRole("combobox", { name: "供应商", exact: true }).click();
  await page.getByRole("option", { name: "DeepSeek", exact: true }).click();
  await wizard.getByRole("button", { name: "下一步" }).click();
  await wizard
    .getByRole("textbox", { name: "API Key", exact: true })
    .fill("fixture-key");
  await wizard.getByRole("button", { name: "继续并获取模型" }).click();
  await expect(wizard.getByText(/服务商返回空目录/)).toBeVisible();
  await wizard.getByRole("button", { name: "手动输入模型 ID" }).click();
  await wizard.getByRole("textbox", { name: "默认模型" }).fill("custom-model");
  await wizard
    .getByRole("checkbox", { name: "同时生成同名默认模型服务" })
    .uncheck();
  await wizard.getByRole("button", { name: "保存并管理模型" }).click();
  await expect(wizard).toHaveCount(0);
  expect(
    await page.evaluate(() => ({
      model: window.autoConnectFixture.manualSaves[0].model,
      createService: window.autoConnectFixture.makeDefaultService,
      probes: window.autoConnectFixture.draftProbes.length,
    })),
  ).toEqual({ model: "custom-model", createService: false, probes: 0 });
});
