import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";

test("model names show the matching bundled family mark on the left and preserve unknown aliases", async ({
  page,
}) => {
  await page.goto("/tests/frontend/provider-model-inventory.html?mode=logos");
  await page.getByRole("tab", { name: "全部模型", exact: true }).click();
  await page
    .getByRole("button", { name: "读取全部模型目录", exact: true })
    .click();
  const table = page.getByRole("region", {
    name: "全部 Provider 模型",
    exact: true,
  });
  const names = [
    ["deepseek-v4-pro[1M]", "deepseek"],
    ["anthropic/claude-sonnet-4", "claude"],
    ["openai/gpt-4.1", "openai"],
    ["google/gemini-2.5-pro", "gemini"],
    ["qwen/qwen3-32b", "qwen"],
    ["mistral/mistral-large", "mistral"],
    ["xai/grok-4", "grok"],
    ["meta-llama/llama-4", "meta"],
  ];
  for (const [id, brand] of names) {
    const cell = table
      .getByRole("cell")
      .filter({ has: page.getByText(id, { exact: true }) })
      .first();
    const mark = cell.locator('span[aria-hidden="true"][style*="mask"]');
    await expect(mark).toBeVisible();
    const expectedSvg = readFileSync(
      `node_modules/@lobehub/icons-static-svg/icons/${brand}.svg`,
      "utf8",
    );
    const result = await mark.evaluate(async (element, expectedSvg) => {
      const style = getComputedStyle(element);
      const mask = style.maskImage || style.webkitMaskImage;
      const asset = mask.match(/^url\("(.*)"\)$/s)![1];
      const response = await fetch(asset);
      const parser = new DOMParser();
      const actual = parser.parseFromString(
        await response.text(),
        "image/svg+xml",
      );
      const expected = parser.parseFromString(expectedSvg, "image/svg+xml");
      const paths = (doc: Document) =>
        [...doc.querySelectorAll("path")].map((path) => path.getAttribute("d"));
      const image = new Image();
      image.src = asset;
      await image.decode();
      const bounds = element.getBoundingClientRect();
      const label = element
        .parentElement!.querySelector("p")!
        .getBoundingClientRect();
      return {
        assetMatches:
          response.ok &&
          actual.documentElement.getAttribute("viewBox") ===
            expected.documentElement.getAttribute("viewBox") &&
          JSON.stringify(paths(actual)) === JSON.stringify(paths(expected)),
        assetLoaded: image.naturalWidth > 0,
        width: bounds.width,
        height: bounds.height,
        onLeft: bounds.right <= label.left,
      };
    }, expectedSvg);
    expect(result.assetMatches).toBe(true);
    expect(result.assetLoaded).toBe(true);
    expect(result.width).toBe(20);
    expect(result.height).toBe(20);
    expect(result.onLeft).toBe(true);
  }
  const unknown = table
    .getByRole("cell")
    .filter({ has: page.getByText("unknown/model-v1", { exact: true }) })
    .first();
  await expect(unknown.locator('span[style*="mask"]')).toHaveCount(0);
  await expect(unknown.locator('svg[aria-hidden="true"]')).toBeVisible();
  await page.setViewportSize({ width: 720, height: 800 });
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await expect(table.locator('span[style*="mask"]').first()).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});
