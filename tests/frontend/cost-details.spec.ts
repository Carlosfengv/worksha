import { test, expect } from "@playwright/test";

test("Session fees group and filter full scope, paginate attempts, and open fee and request detail", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&fees=1#/sessions/claude%3Aa",
  );
  await page.getByRole("tab", { name: "费用", exact: true }).click();
  const detail = page.getByLabel("费用明细", { exact: true });
  await expect(
    detail.getByRole("heading", { name: "逐次调用 · 3" }),
  ).toBeVisible();
  await expect(
    detail.getByRole("button", { name: "请求详情", exact: true }),
  ).toHaveCount(2);
  await detail.getByRole("button", { name: "加载更多费用调用" }).click();
  await expect(
    detail.getByRole("button", { name: "请求详情", exact: true }),
  ).toHaveCount(3);
  await detail.getByRole("button", { name: "费用未知，查看原因" }).click();
  await expect(page.getByRole("dialog")).toContainText("用量");
  await page.keyboard.press("Escape");
  await detail.getByLabel("费用分组", { exact: true }).click();
  await page.getByRole("option", { name: "按模型", exact: true }).click();
  await detail
    .getByRole("button", { name: "model-two · Saved A", exact: true })
    .click();
  await expect(
    detail.getByRole("heading", { name: "逐次调用 · 1" }),
  ).toBeVisible();
  await expect(detail.getByLabel("筛选费用", { exact: true })).toContainText(
    "0.000000004 USD",
  );
  await expect(
    detail.getByRole("button", { name: "请求详情", exact: true }),
  ).toHaveCount(1);
  await detail.getByRole("button", { name: "请求详情", exact: true }).click();
  await expect(page).toHaveURL(/requests\/fee-b$/);
});

test("Project fee detail follows Agent filter and keeps native unmatched fees unknown", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=projects-page&fees=1#/projects/fixture-project?tab=cost",
  );
  await page.getByRole("tab", { name: "费用", exact: true }).click();
  const detail = page.getByLabel("费用明细", { exact: true });
  await expect(detail).toContainText("已关联 3 次网关尝试");
  await detail.getByRole("button", { name: "Saved B", exact: true }).click();
  await expect(
    detail.getByRole("heading", { name: "逐次调用 · 1" }),
  ).toBeVisible();
  await expect(detail.getByLabel("筛选费用", { exact: true })).toContainText(
    "1 次",
  );
  await page.getByLabel("项目 Agent", { exact: true }).click();
  await page.getByRole("option", { name: "客户端未知", exact: true }).click();
  await expect(detail).toContainText("未关联到网关调用，费用未知");
  await expect(
    detail.getByRole("heading", { name: "逐次调用 · 0" }),
  ).toBeVisible();
  await expect(detail).not.toContainText("0.000000004 USD");
  await page.setViewportSize({ width: 390, height: 844 });
  await expect
    .poll(() =>
      page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
    )
    .toBeTruthy();
});

test("Session fee read errors remain unavailable instead of zero", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&fees=1&feeError=1#/sessions/claude%3Aa",
  );
  await page.getByRole("tab", { name: "费用", exact: true }).click();
  const detail = page.getByLabel("费用明细", { exact: true });
  await expect(detail.getByRole("alert")).toContainText("费用来源不可用");
  await expect(detail).toContainText("费用记录暂不可用");
  await expect(
    detail.getByRole("heading", { name: "逐次调用 · 未知" }),
  ).toBeVisible();
  await expect(detail).not.toContainText("0 USD");
  await detail.getByRole("button", { name: "刷新明细" }).click();
  await expect(detail.getByRole("alert")).toContainText("费用来源不可用");
});
