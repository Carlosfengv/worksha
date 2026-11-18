import { expect, test } from "@playwright/test";

test("overview puts trends and live model activity alongside the four-column task table", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&activity=recent-history-attention#/overview",
  );
  const live = page.getByRole("region", { name: "实时模型流向", exact: true });
  const trend = page.getByRole("region", {
    name: "24h 模型调用趋势",
    exact: true,
  });
  const table = page.getByRole("region", { name: "当前活动列表", exact: true });
  await expect(live).toBeVisible();
  await expect(trend).toBeVisible();
  await expect(live.getByLabel("最近 30 秒 Token 速率")).toBeVisible();
  await expect(table.getByRole("columnheader")).toHaveText([
    "当前会话任务",
    "首字延迟 (TTFT)",
    "Token/s",
    "本轮 Token 花费",
  ]);
  expect((await trend.boundingBox())!.y).toBeLessThan(
    (await table.boundingBox())!.y,
  );
  const ttft = table.getByRole("row").nth(1).getByRole("cell").nth(1);
  await expect(ttft.getByRole("progressbar")).toHaveCount(0);
  await expect(page.getByRole("tab", { name: /^执行中/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  const attention = page.getByRole("complementary", { name: "需要关注" });
  const failure = attention.getByRole("link");
  await expect(failure).toHaveAttribute("href", "#/requests/recent-failed");
  await expect(attention).toContainText("HTTP 429");
  await failure.press("Enter");
  await expect(
    page.locator('[data-workspace-tab="request:recent-failed"]'),
  ).toHaveAttribute("data-pinned", "false");
});

test("idle overview collapses live activity and selects recent completions", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&activity=recent-history-expired#/overview",
  );
  const idle = page.getByRole("status", { name: "实时任务汇总" });
  await expect(idle).toContainText("当前无在途");
  await expect(
    page.getByRole("region", { name: "实时模型流向", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByRole("tab", { name: /^最近完成/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});
