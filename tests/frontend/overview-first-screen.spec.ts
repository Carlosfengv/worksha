import { expect, test } from "@playwright/test";

test("native-sized overview keeps live metrics, recent history and name links in one activity table", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.goto(
    "/tests/frontend/tables.html?mode=activity&activity=recent-history-attention#/overview",
  );

  const attention = page.getByRole("complementary", { name: "需要关注" });
  const performance = page.getByRole("region", { name: "实时任务汇总" });
  await expect(
    performance.getByRole("heading", { name: "活动性能明细" }),
  ).toBeVisible();
  const activity = page.getByRole("region", { name: "当前活动列表" });
  const analysis = page.getByRole("region", { name: "调用趋势与占比" });
  const failure = attention.getByRole("link");
  await expect(failure).toHaveCount(1);
  await expect(failure).toHaveAttribute("href", "#/requests/recent-failed");
  await expect(attention).toContainText("HTTP 429");
  const columns = ["任务 / Agent", "阶段", "本轮耗时", "TTFT", "Token/s"];
  await expect(activity.getByRole("columnheader")).toHaveText([
    ...columns,
    "更新时间",
  ]);
  await expect(page.getByRole("tab", { name: /^执行中/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );

  await expect(
    activity.getByRole("row").nth(1).locator("[data-activity-elapsed]"),
  ).toContainText(/秒|分/);
  for (const width of [1280, 720]) {
    await page.setViewportSize({ width, height: 860 });
    expect((await attention.boundingBox())!.y).toBeLessThan(
      (await performance.boundingBox())!.y,
    );
    expect((await performance.boundingBox())!.y).toBeLessThan(
      (await activity.boundingBox())!.y,
    );
    expect((await activity.boundingBox())!.y).toBeLessThan(
      (await analysis.boundingBox())!.y,
    );
    for (const metric of columns.slice(2))
      await expect(
        activity.getByRole("columnheader", { name: metric }),
      ).toBeVisible();
    expect(
      await activity
        .locator("table")
        .evaluate(
          (table) =>
            table.closest(".overflow-x-auto")!.clientWidth >= table.clientWidth,
        ),
    ).toBe(true);
  }

  const recent = page.getByRole("tab", { name: /^最近 3 小时.*6/ });
  await recent.press("Enter");
  await expect(activity.getByRole("link")).toHaveCount(5);
  await activity
    .getByRole("button", { name: "下一页", exact: true })
    .press("Enter");
  await expect(activity.getByRole("link")).toHaveCount(1);
  await expect(
    activity.getByRole("button", { name: /查看|详情|展开指标/ }),
  ).toHaveCount(0);

  await failure.press("Enter");
  await expect(page).toHaveURL(/#\/requests\/recent-failed$/);
  await expect(
    page.getByRole("heading", { name: "请求详情", exact: true }),
  ).toBeVisible();
});
