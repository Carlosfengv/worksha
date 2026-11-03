import { expect, test, type Page } from "@playwright/test";
import type {} from "./cancellation-feedback-harness";

type Purpose = "fixed_route" | "dynamic_binding" | "model_service";
const dialog = (page: Page) => page.getByRole("dialog");
const recovery = (page: Page) =>
  page.getByRole("region", { name: "本机入口结果核查", exact: true });
const snapshot = (page: Page) =>
  page.evaluate(() => window.cancellationFeedback.snapshot());
async function choose(page: Page, purpose: Purpose) {
  if (purpose === "fixed_route")
    await page
      .getByRole("button", { name: "准备配置路由", exact: true })
      .click();
  else if (purpose === "dynamic_binding")
    await page
      .getByRole("button", { name: "接入固定入口", exact: true })
      .click();
  else {
    await page
      .getByRole("combobox", { name: "Claude Code CLI 模型服务", exact: true })
      .click();
    await page.getByRole("option", { name: "隔离服务", exact: true }).click();
  }
}
async function failPrepare(page: Page, purpose: Purpose) {
  await page.goto(
    `/tests/frontend/cancellation-feedback.html?purpose=${purpose}`,
  );
  await choose(page, purpose);
  await dialog(page)
    .getByRole("button", { name: "验证并生成预览", exact: true })
    .click();
  await expect(
    dialog(page).getByText("原准备请求失败，请核查原编号", { exact: true }),
  ).toBeVisible();
  expect((await snapshot(page)).recovery.hints).toHaveLength(1);
}
function stop(page: Page) {
  return recovery(page).getByRole("button", {
    name: "停止这次创建或预览",
    exact: true,
  });
}
for (const purpose of [
  "fixed_route",
  "dynamic_binding",
  "model_service",
] as const) {
  test(`${purpose}: a failed stop preserves the original error; confirmed cancellation clears it without reopening`, async ({
    page,
  }) => {
    await failPrepare(page, purpose);
    await stop(page).click();
    await expect(
      recovery(page).getByText("停止结果尚不可核查", { exact: true }),
    ).toBeVisible();
    await expect(
      dialog(page).getByText("原准备请求失败，请核查原编号", { exact: true }),
    ).toBeVisible();
    await page.evaluate(() =>
      window.cancellationFeedback.setCancelMode("success"),
    );
    await stop(page).click();
    await expect(
      recovery(page).getByText(/原编号已阻止后续创建/),
    ).toBeVisible();
    await expect(
      page.getByText("原准备请求失败，请核查原编号", { exact: true }),
    ).toHaveCount(0);
    await dialog(page)
      .getByRole("button", { name: "取消", exact: true })
      .click();
    await expect(dialog(page)).toBeHidden();
    await expect(
      page.getByText("原准备请求失败，请核查原编号", { exact: true }),
    ).toHaveCount(0);
    const state = await snapshot(page);
    expect(state.recovery.hints).toHaveLength(0);
    expect(
      state.calls.filter((v) => v.name.startsWith("prepare_")),
    ).toHaveLength(1);
  });
  test(`${purpose}: shared recovery clears the matching parent's error after the draft closes`, async ({
    page,
  }) => {
    await failPrepare(page, purpose);
    await dialog(page)
      .getByRole("button", { name: "取消", exact: true })
      .click();
    await expect(dialog(page)).toBeHidden();
    await expect(
      page.getByText("原准备请求失败，请核查原编号", { exact: true }),
    ).toBeVisible();
    await page.evaluate(() =>
      window.cancellationFeedback.setCancelMode("success"),
    );
    await stop(page).click();
    await expect(
      page.getByText("原准备请求失败，请核查原编号", { exact: true }),
    ).toHaveCount(0);
    expect((await snapshot(page)).recovery.hints).toHaveLength(0);
  });
  test(`${purpose}: cancelling an older request cannot clear the current request's error`, async ({
    page,
  }) => {
    await failPrepare(page, purpose);
    const current = (await snapshot(page)).recovery.hints[0].identity;
    const older = await page.evaluate(() => {
      window.cancellationFeedback.setCancelMode("success");
      return window.cancellationFeedback.addOlderHint();
    });
    await recovery(page)
      .getByText(new RegExp(older.operationId))
      .locator("..")
      .getByRole("button", { name: "停止这次创建或预览", exact: true })
      .click();
    await expect(
      dialog(page).getByText("原准备请求失败，请核查原编号", { exact: true }),
    ).toBeVisible();
    expect(
      (await snapshot(page)).recovery.hints.map((v) => v.identity),
    ).toEqual([current]);
  });
}
test("a separate query clears the original error only after it finds a lost cancellation receipt", async ({
  page,
}) => {
  await failPrepare(page, "model_service");
  await page.evaluate(() => window.cancellationFeedback.setCancelMode("lost"));
  await stop(page).click();
  await expect(
    dialog(page).getByText("原准备请求失败，请核查原编号", { exact: true }),
  ).toBeVisible();
  await recovery(page)
    .getByRole("button", { name: "查询原入口结果", exact: true })
    .click();
  await expect(recovery(page).getByText(/原编号已阻止后续创建/)).toBeVisible();
  await expect(
    page.getByText("原准备请求失败，请核查原编号", { exact: true }),
  ).toHaveCount(0);
});
test("a foreign cancellation receipt preserves both the original error and hint", async ({
  page,
}) => {
  await failPrepare(page, "model_service");
  await page.evaluate(() =>
    window.cancellationFeedback.setCancelMode("foreign"),
  );
  await stop(page).click();
  await expect(recovery(page).getByText(/回复或来源不可核查/)).toBeVisible();
  await expect(
    dialog(page).getByText("原准备请求失败，请核查原编号", { exact: true }),
  ).toBeVisible();
  expect((await snapshot(page)).recovery.hints).toHaveLength(1);
});
for (const purpose of ["fixed_route", "model_service"] as const) {
  test(`${purpose}: a later model verification error is not erased by cancelling the original request`, async ({
    page,
  }) => {
    await failPrepare(page, purpose);
    await page.evaluate(() => window.cancellationFeedback.failVerification());
    await dialog(page)
      .getByRole("button", { name: "验证并生成预览", exact: true })
      .click();
    await expect(dialog(page).getByText(/新的模型验证失败/)).toBeVisible();
    await page.evaluate(() =>
      window.cancellationFeedback.setCancelMode("success"),
    );
    await stop(page).click();
    await expect(
      recovery(page).getByText(/原编号已阻止后续创建/),
    ).toBeVisible();
    await expect(dialog(page).getByText(/新的模型验证失败/)).toBeVisible();
    expect(
      (await snapshot(page)).calls.filter((v) => v.name.startsWith("prepare_")),
    ).toHaveLength(1);
  });
}
test("a cancellation reply from a closed draft cannot erase a new draft's error", async ({
  page,
}) => {
  await failPrepare(page, "model_service");
  await page.evaluate(() => window.cancellationFeedback.setCancelMode("hold"));
  await stop(page).click();
  await expect
    .poll(
      async () =>
        (await snapshot(page)).calls.filter(
          (v) => v.name === "cancel_local_token_operation",
        ).length,
    )
    .toBe(1);
  await dialog(page).getByRole("button", { name: "取消", exact: true }).click();
  await choose(page, "model_service");
  await dialog(page)
    .getByRole("button", { name: "验证并生成预览", exact: true })
    .click();
  await expect(
    dialog(page).getByText("原准备请求失败，请核查原编号", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => window.cancellationFeedback.release());
  await expect(
    dialog(page).getByText("原准备请求失败，请核查原编号", { exact: true }),
  ).toBeVisible();
  expect((await snapshot(page)).recovery.hints).toHaveLength(2);
});

test("a rejected changed draft keeps its validation error when the original request is cancelled", async ({
  page,
}) => {
  await failPrepare(page, "fixed_route");
  await dialog(page)
    .getByRole("combobox", { name: "目标模型", exact: true })
    .fill("other-model");
  await page.getByRole("option", { name: /other-model/ }).click();
  await dialog(page)
    .getByRole("button", { name: "验证并生成预览", exact: true })
    .click();
  await expect(dialog(page).getByText(/回复或来源不可核查/)).toBeVisible();
  expect(
    (await snapshot(page)).calls.filter((v) => v.name.startsWith("prepare_")),
  ).toHaveLength(1);
  await page.evaluate(() =>
    window.cancellationFeedback.setCancelMode("success"),
  );
  await stop(page).click();
  await expect(recovery(page).getByText(/原编号已阻止后续创建/)).toBeVisible();
  await expect(dialog(page).getByText(/回复或来源不可核查/)).toBeVisible();
});
