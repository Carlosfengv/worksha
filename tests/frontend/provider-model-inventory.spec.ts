import { expect, test } from "@playwright/test";
import type {} from "./provider-model-inventory-harness";

for (const change of ["database", "recovery"] as const) {
  test(`${change}: identical provider metadata cannot reuse the old catalog`, async ({
    page,
  }) => {
    await page.goto("/tests/frontend/provider-model-inventory.html");
    await page.getByRole("tab", { name: "全部模型", exact: true }).click();
    await page.getByRole("button", { name: "读取全部模型目录" }).click();
    await expect(
      page.getByRole("status").filter({ hasText: "显示" }),
    ).toContainText("65 / 65");
    await page.evaluate((kind) => {
      if (kind === "database")
        window.providerModelsFixture.changeStore(
          "6087597b-0625-42ea-8fd0-e51f6ad16d01",
        );
      else window.providerModelsFixture.recover();
    }, change);
    await expect(
      page.getByRole("status").filter({ hasText: "显示" }),
    ).toContainText("3 / 3");
    await page.getByRole("button", { name: "读取全部模型目录" }).click();
    await expect(
      page.getByRole("status").filter({ hasText: "显示" }),
    ).toContainText("65 / 65");
    expect(
      await page.evaluate(
        () => window.providerModelsFixture.catalogReads.length,
      ),
    ).toBe(4);
  });
  test(`${change}: a late first catalog cannot publish or continue the old batch`, async ({
    page,
  }) => {
    await page.goto(
      "/tests/frontend/provider-model-inventory.html?mode=source-hold",
    );
    await page.getByRole("tab", { name: "全部模型", exact: true }).click();
    await page.getByRole("button", { name: "读取全部模型目录" }).click();
    await expect
      .poll(() =>
        page.evaluate(() => !!window.providerModelsFixture.finishCatalog),
      )
      .toBe(true);
    await page.evaluate((kind) => {
      if (kind === "database")
        window.providerModelsFixture.changeStore(
          "6087597b-0625-42ea-8fd0-e51f6ad16d01",
        );
      else window.providerModelsFixture.recover();
      window.providerModelsFixture.finishCatalog?.();
    }, change);
    await expect(
      page.getByRole("button", { name: "读取全部模型目录" }),
    ).toBeEnabled();
    await expect(
      page.getByRole("status").filter({ hasText: "显示" }),
    ).toContainText("3 / 3");
    expect(
      await page.evaluate(
        () => window.providerModelsFixture.catalogReads.length,
      ),
    ).toBe(1);
  });
}
test("a fresh batch can finish after recovery while the old first reply is still pending", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-model-inventory.html?mode=source-hold",
  );
  await page.getByRole("tab", { name: "全部模型", exact: true }).click();
  const read = page.getByRole("button", { name: "读取全部模型目录" });
  await read.click();
  await expect
    .poll(() =>
      page.evaluate(() => !!window.providerModelsFixture.finishCatalog),
    )
    .toBe(true);
  await page.evaluate(() => window.providerModelsFixture.recover());
  await expect(read).toBeEnabled();
  await read.click();
  const count = page.getByRole("status").filter({ hasText: "显示" });
  await expect(count).toContainText("65 / 65");
  await page.evaluate(() => window.providerModelsFixture.finishCatalog?.());
  await expect(read).toBeEnabled();
  await expect(count).toContainText("65 / 65");
  expect(
    await page.evaluate(() => window.providerModelsFixture.catalogReads.length),
  ).toBe(3);
});
test("missing database identity keeps registered models and prevents catalog discovery", async ({
  page,
}) => {
  await page.goto(
    "/tests/frontend/provider-model-inventory.html?mode=source-missing",
  );
  await page.getByRole("tab", { name: "全部模型", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "读取全部模型目录" }),
  ).toBeDisabled();
  await expect(
    page.getByText("模型目录来源尚未核对", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("status").filter({ hasText: "显示" }),
  ).toContainText("3 / 3");
  expect(
    await page.evaluate(() => window.providerModelsFixture.catalogReads.length),
  ).toBe(0);
});
for (const phase of ["before", "after"] as const) {
  test(`backend changes ${phase} catalog read while the view snapshot stays unchanged`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/frontend/provider-model-inventory.html?mode=source-mismatch-${phase}`,
    );
    await page.getByRole("tab", { name: "全部模型", exact: true }).click();
    await page.getByRole("button", { name: "读取全部模型目录" }).click();
    await expect(
      page.getByText(/草稿属于另一个本机数据库/).first(),
    ).toBeVisible();
    await expect(
      page.getByRole("status").filter({ hasText: "显示" }),
    ).toContainText("3 / 3");
    expect(
      await page.evaluate(
        () => window.providerModelsFixture.catalogReads.length,
      ),
    ).toBe(phase === "before" ? 0 : 1);
  });
}

test("all-models tab isolates provider-model usage and displays directory metadata without inference", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-model-inventory.html");
  await page.getByRole("tab", { name: "全部模型", exact: true }).click();
  const table = page.getByRole("region", {
    name: "全部 Provider 模型",
    exact: true,
  });
  const a = table
    .getByRole("row")
    .filter({ has: page.getByRole("link", { name: "连接 A", exact: true }) })
    .filter({ hasText: "shared" });
  const b = table
    .getByRole("row")
    .filter({ has: page.getByRole("link", { name: "连接 B", exact: true }) });
  await expect(a.getByRole("cell").nth(7)).toHaveText("85");
  await expect(a).toContainText("1,000 · 部分");
  await expect(b.getByRole("cell").nth(7)).toHaveText("1");
  await expect(b).toContainText("0 USD · 实际");
  expect(
    await page.evaluate(() => window.providerModelsFixture.catalogReads),
  ).toHaveLength(0);
  await page.getByRole("button", { name: "读取全部模型目录" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "显示" }),
  ).toContainText("65 / 65");
  await page.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(b).toContainText("200,000 Token");
  await page
    .getByRole("textbox", { name: "搜索模型或 Provider" })
    .fill("shared");
  await expect(a).toContainText("131,072 Token");
  await expect(a).toContainText("8,192 Token");
  await expect(a).toContainText("工具调用：支持");
  await expect(a).toContainText("图片输入：不支持");
  await expect(b).toContainText("200,000 Token");
  await expect(b).toContainText("输入上限");
  await expect(b).toContainText("工具调用：未知");
  await expect(
    page.getByRole("status").filter({ hasText: "显示" }),
  ).toContainText("2 / 65");
  await page
    .getByRole("textbox", { name: "搜索模型或 Provider" })
    .fill("remote-model-061");
  await expect(table).toContainText("remote-model-061");
  await expect(table).toContainText("目录可选");
  await expect(
    table.getByRole("row").nth(1).getByRole("cell").nth(7),
  ).toHaveText("0");
  await page
    .getByRole("textbox", { name: "搜索模型或 Provider" })
    .fill("disabled");
  await expect(table).toContainText("已停用");
  await expect(table).toContainText("未报告");
  await page
    .getByRole("textbox", { name: "搜索模型或 Provider" })
    .fill("no-such-model");
  await expect(page.getByText("没有匹配的模型", { exact: true })).toBeVisible();
});

test("catalog loading, per-provider denial and retry preserve other providers and last snapshots", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-model-inventory.html?mode=loading");
  await page.getByRole("tab", { name: "全部模型", exact: true }).click();
  await page
    .getByRole("textbox", { name: "搜索模型或 Provider" })
    .fill("shared");
  await page.getByRole("button", { name: "读取全部模型目录" }).click();
  await expect(
    page.getByText("读取中…", { exact: true }).first(),
  ).toBeVisible();
  await page.evaluate(() => window.providerModelsFixture.finishCatalog?.());
  await expect(page.getByText("131,072 Token")).toBeVisible();
  await page.evaluate(() => {
    window.providerModelsFixture.denied = true;
    window.providerModelsFixture.usageFailed = true;
  });
  await page.getByRole("button", { name: "读取全部模型目录" }).click();
  await expect(page.getByText(/模型目录鉴权被拒绝/)).toBeVisible();
  await expect(page.getByText("200,000 Token")).toBeVisible();
  await expect(page.getByText(/上次快照 ·/)).toBeVisible();
  await page.getByRole("button", { name: "刷新用量" }).click();
  await expect(page.getByText(/保留上次快照/)).toBeVisible();
  await page.evaluate(() => {
    window.providerModelsFixture.denied = false;
    window.providerModelsFixture.usageFailed = false;
  });
  await page.getByRole("button", { name: "读取全部模型目录" }).click();
  await expect(page.getByText(/模型目录鉴权被拒绝/)).toHaveCount(0);
  await page.getByRole("button", { name: "重试调用统计" }).click();
  await expect(page.getByText("调用统计未更新", { exact: true })).toHaveCount(
    0,
  );
});

test("source and provider filters retain unknown permission state and narrow layout owns horizontal scroll", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 700 });
  await page.goto("/tests/frontend/provider-model-inventory.html");
  await page.getByRole("tab", { name: "连接与调用", exact: true }).focus();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("tab", { name: "全部模型", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await page.getByRole("button", { name: "读取全部模型目录" }).click();
  await page
    .getByRole("combobox", { name: "模型 Provider 筛选", exact: true })
    .click();
  await page.getByRole("option", { name: "连接 B", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "显示" }),
  ).toContainText("1 / 65");
  await page
    .getByRole("combobox", { name: "供应商调用来源", exact: true })
    .click();
  await page.getByRole("option", { name: "原生模型响应", exact: true }).click();
  await expect(
    page.getByText("原生会话来源未接入", { exact: true }),
  ).toBeVisible();
  const table = page.getByRole("region", {
    name: "全部 Provider 模型",
    exact: true,
  });
  await expect(
    table.getByRole("row").nth(1).getByRole("cell").nth(7),
  ).toHaveText("未知");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("partial coverage, older services and missing providers do not fabricate zero usage", async ({
  page,
}) => {
  for (const mode of ["partial", "legacy", "empty"]) {
    await page.goto(
      `/tests/frontend/provider-model-inventory.html?mode=${mode}`,
    );
    await page.getByRole("tab", { name: "全部模型", exact: true }).click();
    if (mode === "empty") {
      await expect(
        page.getByText("尚未添加 Provider", { exact: true }),
      ).toBeVisible();
      continue;
    }
    if (mode === "legacy")
      await expect(page.getByText(/尚未提供按连接划分/)).toBeVisible();
    else {
      await page.getByRole("button", { name: "读取全部模型目录" }).click();
      await page
        .getByRole("textbox", { name: "搜索模型或 Provider" })
        .fill("remote-model-061");
    }
    const table = page.getByRole("region", {
      name: "全部 Provider 模型",
      exact: true,
    });
    await expect(
      table.getByRole("row").nth(1).getByRole("cell").nth(7),
    ).toHaveText("未知");
  }
});

test("same-authority refresh preserves the committed model page while metadata and directories are rechecked", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-model-inventory.html");
  await page.getByRole("tab", { name: "全部模型", exact: true }).click();
  const read = page.getByRole("button", { name: "读取全部模型目录" });
  await read.click();
  await expect(
    page.getByRole("status").filter({ hasText: "显示" }),
  ).toContainText("65 / 65");
  await page.getByRole("button", { name: "下一页", exact: true }).click();
  const first = page
    .getByRole("region", { name: "全部 Provider 模型", exact: true })
    .getByRole("row")
    .nth(1);
  const firstModel = first.getByRole("cell").first();
  const before = await firstModel.textContent();
  await read.click();
  await expect(read).toBeEnabled();
  await expect(firstModel).toHaveText(before!);
  await expect(
    page.getByRole("button", { name: "下一页", exact: true }),
  ).toBeDisabled();
});
