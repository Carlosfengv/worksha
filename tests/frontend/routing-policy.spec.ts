import { expect, test } from "@playwright/test";
test("inspect, explicit install, activate and bind the selected version with explicit failure choice", async ({
  page,
}) => {
  await page.goto("/tests/frontend/routing-policy.html");
  await expect(
    page.getByRole("region", { name: "外部路由策略" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "选择策略包", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("公钥：");
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.calls.some((c) => c.kind === "install"),
    ),
  ).toBe(false);
  await page
    .getByRole("button", { name: "信任此发布者并安装", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("row").filter({ hasText: "反向候选排序" }).click();
  await page.getByRole("button", { name: "启用此版本", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "确认", exact: true })
    .click();
  await expect(
    page.getByRole("row").filter({ hasText: "反向候选排序" }),
  ).toContainText("已启用");
  await page.getByRole("combobox", { name: "模型服务", exact: true }).click();
  await page.getByRole("option", { name: "编程服务", exact: true }).click();
  await page
    .getByRole("button", { name: "绑定当前服务版本", exact: true })
    .click();
  await expect(page.getByText(/绑定已授权/)).toBeVisible();
  const input = await page.evaluate(() =>
    window.routingPolicyFixture.calls
      .slice()
      .reverse()
      .find((c) => c.kind === "bind"),
  );
  expect(input).toMatchObject({
    kind: "bind",
    failureMode: "reject",
    serviceVersionId: "00000000-0000-4000-8000-000000000003",
    expectedBindingRevision: null,
  });
  await page.getByRole("combobox", { name: "排序失败时", exact: true }).click();
  await page
    .getByRole("option", { name: "首次发送前使用内置策略", exact: true })
    .click();
  await page
    .getByRole("button", { name: "绑定当前服务版本", exact: true })
    .click();
  await expect(page.getByText(/失败时使用内置策略/)).toBeVisible();
  await page
    .getByRole("button", { name: "解除绑定，使用内置策略", exact: true })
    .click();
  await expect(
    page.getByText("当前未绑定外部策略", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    window.routingPolicyFixture.malformedRegistry = true;
  });
  await page.getByRole("button", { name: "重新读取策略", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("当前绑定状态未知");
  await expect(
    page.getByRole("button", { name: "绑定当前服务版本", exact: true }),
  ).toBeDisabled();
});
test("cancelled picker does not install and late picker cannot reopen after unmount", async ({
  page,
}) => {
  await page.goto("/tests/frontend/routing-policy.html");
  await page.evaluate(() => {
    window.routingPolicyFixture.cancelPicker = true;
  });
  await page.getByRole("button", { name: "选择策略包", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.evaluate(() => {
    window.routingPolicyFixture.cancelPicker = false;
    window.routingPolicyFixture.holdPicker = true;
  });
  await page.getByRole("button", { name: "选择策略包", exact: true }).click();
  await page.evaluate(() => {
    window.routingPolicyFixture.unmount();
    window.routingPolicyFixture.releasePicker();
  });
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.calls.some((c) => c.kind === "install"),
    ),
  ).toBe(false);
});
test("foreign store responses remain an error and minimum layout has no horizontal overflow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/routing-policy.html");
  await page.evaluate(() => {
    window.routingPolicyFixture.wrongScope = true;
  });
  await page.getByRole("button", { name: "重新读取策略", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("策略响应范围已变化");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "重新读取策略", exact: true }),
  ).toBeEnabled();
});
test("unknown publication requires confirmation and resumes only its original identity", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 560 });
  await page.goto("/tests/frontend/routing-policy.html");
  await page.evaluate(() => window.routingPolicyFixture.seedUnknown());
  await page.getByRole("button", { name: "重新读取策略", exact: true }).click();
  await page.getByRole("button", { name: "继续原发布", exact: true }).click();
  await expect(page.getByRole("dialog")).toContainText("原服务配置");
  await expect(page.getByRole("dialog")).toContainText(
    "00000000-0000-4000-8000-000000000020",
  );
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.calls.some(
        (c) => c.kind === "resume-publish",
      ),
    ),
  ).toBe(false);
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "取消", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "继续原发布", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "按原编号继续提交", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const submits = await page.evaluate(() =>
    window.routingPolicyFixture.calls.filter(
      (c) => c.kind === "resume-publish",
    ),
  );
  expect(submits).toEqual([
    {
      kind: "resume-publish",
      storeId: "00000000-0000-4000-8000-000000000001",
      operationId: "00000000-0000-4000-8000-000000000020",
    },
  ]);
  await expect(
    page.getByRole("button", { name: "继续原发布", exact: true }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.calls.some((c) => c.kind === "publish"),
    ),
  ).toBe(false);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
test("resume rejects a foreign receipt and keeps the error visible in its confirmation", async ({
  page,
}) => {
  await page.goto("/tests/frontend/routing-policy.html");
  await page.evaluate(() => window.routingPolicyFixture.seedUnknown());
  await page.getByRole("button", { name: "重新读取策略", exact: true }).click();
  await page.getByRole("button", { name: "继续原发布", exact: true }).click();
  await page.evaluate(() => {
    window.routingPolicyFixture.wrongScope = true;
  });
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "按原编号继续提交", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page.getByRole("dialog").getByText(/策略响应范围已变化/),
  ).toBeVisible();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("button", { name: "按原编号继续提交", exact: true }),
  ).toBeDisabled();
  expect(
    await page.evaluate(
      () =>
        window.routingPolicyFixture.calls.filter(
          (c) => c.kind === "resume-publish",
        ).length,
    ),
  ).toBe(1);
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.calls.some((c) => c.kind === "publish"),
    ),
  ).toBe(false);
});
test("a still-unknown resume stays visible and never retries automatically", async ({
  page,
}) => {
  await page.goto("/tests/frontend/routing-policy.html");
  await page.evaluate(() => {
    window.routingPolicyFixture.seedUnknown();
    window.routingPolicyFixture.resumeStillUnknown = true;
  });
  await page.getByRole("button", { name: "重新读取策略", exact: true }).click();
  await page.getByRole("button", { name: "继续原发布", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "按原编号继续提交", exact: true })
    .click();
  await expect(
    page
      .getByRole("dialog")
      .getByText("原发布结果仍未确认，请保留原编号继续核查。"),
  ).toBeVisible();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("button", { name: "按原编号继续提交", exact: true }),
  ).toBeEnabled();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "取消", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        window.routingPolicyFixture.calls.filter(
          (c) => c.kind === "resume-publish",
        ).length,
    ),
  ).toBe(1);
  expect(
    await page.evaluate(() =>
      window.routingPolicyFixture.calls.some((c) => c.kind === "publish"),
    ),
  ).toBe(false);
});
