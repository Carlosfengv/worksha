import { test, expect } from "@playwright/test";

test("取消原生插件包选择不会安装、启用或留下确认弹层", async ({ page }) => {
  await page.goto("/tests/frontend/external-plugin.html?mode=picker-cancel");
  const choose = page.getByRole("button", { name: "选择插件包" });
  await choose.click();
  await expect(page.locator("#calls")).toHaveText('["package-picker-cancel"]');
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByText("尚未安装外部插件")).toBeVisible();
  await expect(choose).toBeEnabled();
});

test("文件选择失败可重试，重新检查包必须重新确认发布者", async ({ page }) => {
  await page.goto(
    "/tests/frontend/external-plugin.html?mode=picker-error-retry",
  );
  const choose = page.getByRole("button", { name: "选择插件包" });
  await choose.click();
  await expect(page.getByText("原生文件选择未完成")).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await choose.click();
  await expect(page.getByRole("button", { name: "安装此包" })).toBeDisabled();
  await page.getByRole("checkbox").check();
  await page.keyboard.press("Escape");
  await choose.click();
  await expect(page.getByRole("checkbox")).not.toBeChecked();
  await expect(page.getByRole("button", { name: "安装此包" })).toBeDisabled();
  await expect(page.locator("#calls")).toHaveText('["package-picker-error"]');
});

for (const mode of ["source-cancel", "source-error-retry"]) {
  test(`原生来源重选 ${mode} 保留原授权及完整快照`, async ({ page }) => {
    await page.goto(`/tests/frontend/external-plugin.html?mode=${mode}`);
    await page.getByRole("button", { name: "手动刷新会话" }).click();
    const choose = page.getByRole("button", { name: "选择并授权 JSONL" });
    await choose.click();
    await expect(
      page.getByText("来源文件：existing.jsonl", { exact: false }),
    ).toBeVisible();
    await expect(page.getByText(/9007199254741000 Token/)).toBeVisible();
    await expect(choose).toBeEnabled();
    await expect(page.locator("#calls")).toHaveText(
      JSON.stringify([
        "refresh",
        mode === "source-cancel"
          ? "source-picker-cancel"
          : "source-picker-error",
      ]),
    );
    if (mode === "source-error-retry") {
      await expect(page.getByText("无法读取所选文件")).toBeVisible();
      await choose.click();
      await expect(
        page.getByText("来源文件：fixture.jsonl", { exact: false }),
      ).toBeVisible();
      await expect(page.locator("#calls")).toHaveText(
        '["refresh","source-picker-error","authorize"]',
      );
      await expect(page.getByText("无法读取所选文件")).toHaveCount(0);
    }
  });
}

test("外部安装、启用、来源授权彼此独立，弹层支持键盘取消", async ({ page }) => {
  await page.goto("/tests/frontend/external-plugin.html");
  await page.getByRole("button", { name: "选择插件包" }).click();
  await expect(page.getByRole("button", { name: "安装此包" })).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "选择插件包" }).click();
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: "安装此包" }).click();
  await expect(
    page.getByRole("button", { name: "选择并授权 JSONL" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "启用此版本", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "确认启用此版本", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "选择并授权 JSONL" }),
  ).toBeVisible();
  await expect(page.locator("#calls")).toHaveText('["install","enable"]');
  await page.getByRole("button", { name: "选择并授权 JSONL" }).click();
  await expect(
    page.getByText("来源文件：fixture.jsonl", { exact: false }),
  ).toBeVisible();
});
test("手动刷新、精确整数、按需纯文本正文和撤销后清空", async ({ page }) => {
  await page.goto("/tests/frontend/external-plugin.html?mode=active");
  await expect(
    page.getByRole("link", { name: "插件管理", exact: true }),
  ).toHaveAttribute("href", "#/plugins");
  await expect(
    page.getByText("已授权，尚未读取。请手动刷新会话。"),
  ).toBeVisible();
  await expect(page.locator("#calls")).toHaveText("[]");
  await page.getByRole("button", { name: "手动刷新会话" }).click();
  await expect(page.getByText(/9007199254741000 Token/)).toBeVisible();
  await page.getByText("离线扩展会话", { exact: true }).click();
  await expect(
    page.getByRole("article", { name: "扩展会话正文" }),
  ).toContainText("<img src=x onerror=alert(1)>");
  await expect(
    page.getByRole("article", { name: "扩展会话正文" }).locator("img"),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "撤销资源授权", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "确认撤销资源授权", exact: true })
    .click();
  await expect(page.getByRole("article", { name: "扩展会话正文" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("button", { name: "手动刷新会话" }),
  ).toBeDisabled();
});
test("撤销后延迟的正文响应不能重新出现，窄窗口保留操作", async ({ page }) => {
  await page.setViewportSize({ width: 720, height: 650 });
  await page.goto("/tests/frontend/external-plugin.html?mode=race");
  await page.getByRole("button", { name: "手动刷新会话" }).click();
  await page.getByText("离线扩展会话", { exact: true }).click();
  await page.getByRole("button", { name: "撤销资源授权", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "确认撤销资源授权", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "手动刷新会话" }),
  ).toBeDisabled();
  await page.waitForTimeout(500);
  await expect(page.getByRole("article", { name: "扩展会话正文" })).toHaveCount(
    0,
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
for (const mode of ["removed", "error"]) {
  test(`扩展失效状态 ${mode} 有返回入口`, async ({ page }) => {
    await page.goto(`/tests/frontend/external-plugin.html?mode=${mode}`);
    await expect(
      page.getByRole("button", { name: "返回插件管理" }),
    ).toBeVisible();
    if (mode === "error")
      await expect(page.getByText("目录读取失败")).toBeVisible();
    await page.getByRole("button", { name: "返回插件管理" }).click();
    await expect(page).toHaveURL(/#\/plugins$/);
  });
}
