import { expect, test } from "@playwright/test";

test("Agent keeps response and gateway costs separate, including actual zero and tiny estimates", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&fees=1#/agents/claude-code-cli",
  );
  const cost = page.getByLabel("关联费用", { exact: true });
  await expect(cost).toContainText("0 USD");
  await expect(cost.getByTitle("0.000000004 USD", { exact: true })).toHaveText(
    "< 0.0001 USD",
  );
  await expect(cost).toContainText("原生覆盖未确认");
  await cost.getByRole("button", { name: "费用统计依据" }).click();
  await expect(cost).toContainText("这些响应不再增加费用或计价未知次数");
  await page.getByRole("tab", { name: "用量与额度", exact: true }).click();
  const usage = page.getByLabel("模型调用费用", { exact: true });
  await expect(usage).toContainText("计价未知");
  await expect(usage).toContainText("2 次");
  await page.getByLabel("模型用量来源").click();
  await page
    .getByRole("option", { name: "本地网关 · 上游服务", exact: true })
    .click();
  await expect(usage.getByTitle("0.000000004 USD", { exact: true })).toHaveText(
    "< 0.0001 USD",
  );
  await expect(usage).toContainText("1 次");
  await page.getByRole("button", { name: "用量详情 model-cost" }).click();
  await expect(page.getByRole("dialog")).toContainText("0.000000004 USD");
});

test("Session fee view does not reuse another Session's known cost and links to scoped gateway requests", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&fees=1#/sessions/claude%3Aa",
  );
  await page.getByRole("tab", { name: "费用", exact: true }).click();
  await expect(
    page
      .getByLabel("关联费用", { exact: true })
      .getByTitle("0.000000004 USD", { exact: true }),
  ).toHaveText("< 0.0001 USD");
  await page.getByRole("button", { name: "查看网关调用", exact: true }).click();
  await expect(page).toHaveURL(
    /requests\?source=gateway&range=all&session=claude%3Aa/,
  );
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&fees=1#/sessions/claude%3Ab",
  );
  await page.getByRole("tab", { name: "费用", exact: true }).click();
  const cost = page.getByLabel("关联费用", { exact: true });
  await expect(cost).toContainText("未关联到网关调用，费用未知");
  await expect(cost).not.toContainText("0.000000004");
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBeTruthy();
});

test("Project fees retain scope and unknown coverage in the overview and list", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=projects-page&fees=1#/projects/fixture-project",
  );
  const cost = page.getByLabel("关联费用", { exact: true });
  await expect(cost.getByTitle("0.000000004 USD", { exact: true })).toHaveText(
    "< 0.0001 USD",
  );
  await page.getByLabel("项目 Agent", { exact: true }).click();
  await page.getByRole("option", { name: "客户端未知", exact: true }).click();
  await expect(cost).toContainText("未关联到网关调用，费用未知");
  await expect(cost).not.toContainText("0.000000004");
  await page.getByRole("button", { name: "全部项目", exact: true }).click();
  await expect(
    page.getByText("关联费用", { exact: true }).first(),
  ).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBeTruthy();
});
