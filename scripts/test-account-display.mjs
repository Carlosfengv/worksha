import assert from "node:assert/strict";
import {
  accountsForProvider,
  resetCountdown,
  selectBillingAccount,
} from "../src/features/accounts/display.ts";
const accounts = [
  {
    id: "api-a",
    name: "DeepSeek",
    providerId: "deepseek-a",
    accountType: "api",
  },
  {
    id: "api-b",
    name: "DeepSeek",
    providerId: "deepseek-b",
    accountType: "api",
  },
  { id: "native-codex", providerId: null, accountType: "subscription" },
  {
    id: "subscription-a",
    providerId: "deepseek-a",
    accountType: "subscription",
  },
];
assert.deepEqual(
  accountsForProvider(accounts, "deepseek-a").map((a) => a.id),
  ["api-a", "subscription-a"],
);
assert.deepEqual(
  accountsForProvider(accounts, "deepseek-b").map((a) => a.id),
  ["api-b"],
);
assert.deepEqual(accountsForProvider(accounts, "unknown-provider"), []);
assert.deepEqual(accountsForProvider(accounts, ""), []);
assert.deepEqual(accountsForProvider([], "deepseek-a"), []);
assert.deepEqual(accountsForProvider(accounts), accounts);
const agentAccounts = accounts.map((account) => ({
  ...account,
  declaredFor: account.id === "native-codex" ? ["codex"] : [],
}));
assert.equal(
  selectBillingAccount(agentAccounts, "", "codex").id,
  "native-codex",
);
assert.equal(selectBillingAccount(agentAccounts, "", "opencode"), undefined);
assert.equal(selectBillingAccount(agentAccounts, "", "claude"), undefined);
assert.equal(
  selectBillingAccount(agentAccounts, "api-b", "opencode").id,
  "api-b",
);
assert.equal(
  selectBillingAccount(
    accountsForProvider(agentAccounts, "unknown-provider"),
    "native-codex",
  ),
  undefined,
);
assert.equal(
  selectBillingAccount(
    accountsForProvider(agentAccounts, "deepseek-a"),
    "native-codex",
  ).id,
  "api-a",
);
const reset = 2_000_000_000;
assert.equal(resetCountdown(reset, (reset - 3661) * 1000), "01:01:01");
assert.equal(resetCountdown(reset, (reset - 86401) * 1000), "1 天 00:00:01");
assert.equal(
  resetCountdown(reset, reset * 1000),
  "已到重置时间 · 等待用量更新",
);
assert.equal(
  resetCountdown(reset, (reset + 9999) * 1000),
  "已到重置时间 · 等待用量更新",
);
assert.equal(resetCountdown(null, reset * 1000), "来源未提供重置时间");
assert.equal(resetCountdown(reset + 86400, (reset + 1) * 1000), "23:59:59");
console.log(
  "通过：按连接隔离同名供应商账户、保留关联订阅、不关联的账户不回退、Agent 账户目录兼容；独立窗口、跨日计时、到点等待更新、恢复后按绝对时间计算与缺失重置时间。",
);
