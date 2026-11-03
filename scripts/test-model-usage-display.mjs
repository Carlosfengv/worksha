import assert from "node:assert/strict";
import { usageNumber } from "../src/features/agents/usage-display.ts";
assert.equal(usageNumber(undefined, "totalTokens"), "未报告");
const totals = {
  attempts: 2,
  usage: { totalTokens: 0 },
  reported: { totalTokens: 2 },
};
assert.equal(usageNumber(totals, "totalTokens"), "0");
assert.equal(
  usageNumber(
    { ...totals, usage: { totalTokens: 1200 }, reported: { totalTokens: 1 } },
    "totalTokens",
  ),
  "1,200 · 部分",
);
assert.equal(
  usageNumber({ ...totals, usage: { totalTokens: null } }, "totalTokens"),
  "未报告",
);
assert.equal(
  usageNumber({ ...totals, usage: { totalTokens: NaN } }, "totalTokens"),
  "未报告",
);
assert.equal(usageNumber(totals, "cachedInputTokens"), "未报告");
console.log("通过：模型用量零值、未知、部分报告与缺失缓存字段分别显示。");
