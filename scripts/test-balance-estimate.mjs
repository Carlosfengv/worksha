import assert from "node:assert/strict";
import {
  balanceDuration,
  estimateProviderBalance,
} from "../src/features/accounts/balance-estimate.ts";

const now = Date.parse("2026-10-25T06:00:00Z");
const account = {
  id: "account-a",
  providerId: "provider-a",
  accountType: "api",
  state: "ready",
  error: null,
  snapshot: {
    accountType: "api",
    asOf: new Date(now).toISOString(),
    validForSeconds: 300,
    money: [{ kind: "balance", amount: "8.55", currency: "CNY" }],
  },
};
const usage = {
  query: { providerId: "provider-a", source: "gateway", days: 7 },
  asOf: new Date(now).toISOString(),
  sourceAsOf: new Date(now).toISOString(),
  sourceState: "ready",
  missingTime: 0,
  selected: {
    calls: 2,
    inProgress: 0,
    cost: {
      actual: [{ currency: "CNY", amount: 3, calls: 1 }],
      estimated: [{ currency: "CNY", amount: 4, calls: 1 }],
      unknownCalls: 0,
    },
  },
  // The full selected aggregate must be used even when the current call page is empty.
  calls: [],
  totalCalls: 2,
  summary: { calls: 999999 },
};
function estimate(
  edit = () => {},
  time = now,
  accountError = false,
  usageError = false,
) {
  const a = structuredClone(account),
    u = structuredClone(usage);
  edit(a, u);
  const before = structuredClone({ a, u });
  const result = estimateProviderBalance(
    a,
    "provider-a",
    u,
    time,
    accountError,
    usageError,
  );
  assert.deepEqual({ a, u }, before, "计算不改写输入快照");
  return result;
}
const ready = estimate();
assert.deepEqual(ready.reasons, []);
assert.equal(ready.sevenDaySpend, 7);
assert.equal(ready.dailySpend, 1);
assert.equal(ready.remainingDays, 8.55);
assert.equal(balanceDuration(ready.remainingDays), "约 8.6 天");
assert.equal(
  estimate((a) => {
    a.snapshot.money[0].amount = "0";
  }).remainingDays,
  0,
);
assert.equal(balanceDuration(0), "余额已用完");
assert.equal(balanceDuration(null), "未知");
assert.equal(balanceDuration(0.5), "约 12 小时");
assert.equal(balanceDuration(0.001), "不足 1 小时");
const unknown = (result, message) => {
  assert.equal(result.remainingDays, null);
  assert.equal(result.dailySpend, null);
  assert.ok(
    result.reasons.some((reason) => reason.includes(message)),
    result.reasons.join(" / "),
  );
};
unknown(
  estimate((a, u) => {
    u.selected.cost.estimated[0].currency = "USD";
  }),
  "币种不一致",
);
unknown(
  estimate((a, u) => {
    u.selected.cost.unknownCalls = 1;
    u.selected.calls++;
  }),
  "1 次调用费用未知",
);
unknown(
  estimate((a, u) => {
    u.selected.inProgress = 1;
  }),
  "进行中",
);
unknown(
  estimate((a, u) => {
    u.missingTime = 1;
  }),
  "时间不明",
);
unknown(
  estimate((a, u) => {
    u.selected.cost.estimated = [];
    u.selected.calls = 1;
  }),
  "样本不足",
);
unknown(
  estimate((a, u) => {
    u.selected.calls = 99;
  }),
  "覆盖数量",
);
unknown(
  estimate((a, u) => {
    u.selected.cost.actual[0].amount = 0;
    u.selected.cost.estimated[0].amount = 0;
  }),
  "费用为零",
);
for (const bad of [NaN, Infinity, -1])
  unknown(
    estimate((a, u) => {
      u.selected.cost.actual[0].amount = bad;
    }),
    "无效",
  );
for (const bad of [null, "", " ", "NaN", "8e3", "-1", "9007199254740993"])
  unknown(
    estimate((a) => {
      a.snapshot.money[0].amount = bad;
    }),
    "有效余额",
  );
unknown(
  estimate((a) => {
    a.snapshot.money.push({ ...a.snapshot.money[0] });
  }),
  "单一",
);
unknown(
  estimate((a) => {
    a.snapshot.money[0].currency = "";
  }),
  "币种",
);
unknown(
  estimate((a) => {
    a.snapshot = null;
  }),
  "尚未读取",
);
unknown(
  estimate((a) => {
    a.state = "stale";
  }),
  "已过期",
);
unknown(
  estimate((a) => {
    a.state = "refresh_failed";
  }),
  "读取失败",
);
unknown(
  estimate((a) => {
    a.state = "refreshing";
  }),
  "未就绪",
);
unknown(
  estimate((a) => {
    a.snapshot.accountType = "subscription";
  }),
  "类型不一致",
);
unknown(
  estimate((a) => {
    a.snapshot.asOf = "invalid";
  }),
  "时间",
);
unknown(
  estimate((a) => {
    a.snapshot.asOf = new Date(now + 1).toISOString();
  }),
  "时间",
);
unknown(
  estimate((a) => {
    a.snapshot.validForSeconds = 1;
  }, now + 1001),
  "余额快照已过期",
);
unknown(
  estimate(() => {}, now, true),
  "余额读取失败",
);
unknown(
  estimate(() => {}, now, false, true),
  "调用费用未更新",
);
for (const state of ["partial", "disabled", "failed"])
  unknown(
    estimate((a, u) => {
      u.sourceState = state;
    }),
    "来源未就绪",
  );
for (const key of ["asOf", "sourceAsOf"])
  unknown(
    estimate((a, u) => {
      u[key] = new Date(now - 30001).toISOString();
    }),
    "费用快照已过期",
  );
for (const patch of [
  { providerId: "provider-b" },
  { days: 1 },
  { source: "native" },
  { agent: "claude" },
  { model: "one" },
  { cursor: "page-2" },
])
  unknown(
    estimate((a, u) => {
      Object.assign(u.query, patch);
    }),
    "范围不一致",
  );
unknown(
  estimate((a) => {
    a.providerId = "provider-b";
  }),
  "尚未确认",
);
unknown(
  estimate((a) => {
    a.accountType = "subscription";
  }),
  "尚未确认",
);
unknown(
  estimateProviderBalance(account, "provider-a", null, now, false, false),
  "尚未取得",
);
unknown(
  estimateProviderBalance(account, "provider-a", null, now, false, true),
  "读取失败",
);
unknown(
  estimate((a, u) => {
    u.selected = null;
  }),
  "暂无可确认",
);
console.log(
  "通过：余额消耗估算公式、同币种实际与估算费用、零余额、样本不足、未知费用、源状态 / 时效、账户隔离与分页范围；不换汇、不将未知变零。",
);
