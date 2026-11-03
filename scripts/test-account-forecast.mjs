import assert from "node:assert/strict";
import {
  providerQuotaForecasts,
  quotaOutcome,
  quotaPercent,
} from "../src/features/accounts/forecast.ts";

const now = Date.parse("2026-10-25T04:00:00Z");
const account = {
  id: "linked-account",
  name: "同名账户",
  accountType: "subscription",
  providerId: "provider-a",
  state: "ready",
  error: null,
  snapshot: {
    asOf: new Date(now).toISOString(),
    validForSeconds: 300,
    windows: [
      {
        id: "five-hour",
        label: "5 小时窗口",
        usedPercent: 80,
        remainingPercent: 20,
        resetsAt: now / 1000 + 180 * 60,
        durationMins: 300,
      },
    ],
  },
};
function forecast(edit = () => {}, time = now, error = false) {
  const copy = structuredClone(account);
  edit(copy, copy.snapshot.windows[0]);
  const before = structuredClone(copy);
  const result = providerQuotaForecasts(copy, "provider-a", time, error);
  assert.deepEqual(copy, before, "预测不得修改账户快照");
  return result[0];
}
const risk = forecast();
assert.equal(risk.state, "ready");
assert.equal(risk.projectedPercent, 200);
assert.equal(risk.runsOutAt, now + 30 * 60_000);
assert.equal(
  forecast(() => {}, now + 60_000).runsOutAt,
  risk.runsOutAt,
  "时钟前进不能将旧百分比当作新采样，改变预测速度",
);
assert.equal(
  forecast((a, w) => {
    w.usedPercent = 0;
    w.remainingPercent = 100;
  }).projectedPercent,
  0,
);
assert.equal(
  forecast((a, w) => {
    w.usedPercent = 0;
  }).runsOutAt,
  null,
);
assert.equal(
  forecast((a, w) => {
    w.usedPercent = 20;
  }).projectedPercent,
  50,
);
assert.equal(
  forecast((a, w) => {
    w.usedPercent = 40;
  }).runsOutAt,
  null,
  "恰在重置时到达 100% 不算提前耗尽",
);
assert.equal(
  quotaOutcome(
    forecast((a, w) => {
      w.usedPercent = 100;
    }),
  ),
  "快照时额度已用完",
);
for (const usedPercent of [null, NaN, Infinity, -1, 101]) {
  const result = forecast((a, w) => {
    w.usedPercent = usedPercent;
  });
  assert.equal(result.usedPercent, null);
  assert.equal(result.state, "unknown");
  assert.equal(result.projectedPercent, null);
}
assert.equal(
  forecast((a, w) => {
    w.remainingPercent = null;
  }).remainingPercent,
  null,
);
assert.equal(
  forecast((a, w) => {
    w.resetsAt = null;
  }).state,
  "unknown",
);
assert.equal(
  forecast((a, w) => {
    w.resetsAt = now / 1000;
  }).state,
  "stale",
);
for (const duration of [null, 0, -1, NaN, Infinity])
  assert.equal(
    forecast((a, w) => {
      w.durationMins = duration;
    }).state,
    "insufficient",
  );
assert.equal(
  forecast((a, w) => {
    w.resetsAt = now / 1000 + 300 * 60;
  }).state,
  "insufficient",
);
for (const state of ["stale", "refresh_failed"]) {
  const result = forecast((a) => {
    a.state = state;
  });
  assert.equal(result.state, "stale");
  assert.equal(result.usedPercent, 80);
  assert.equal(result.runsOutAt, null);
}
assert.equal(forecast(() => {}, now + 300_001).state, "stale");
assert.equal(forecast(() => {}, now, true).state, "stale");
for (const state of [
  "refreshing",
  "failed",
  "unsupported",
  "not_available",
  "permission_denied",
])
  assert.equal(
    forecast((a) => {
      a.state = state;
    }).state,
    "unknown",
  );
assert.equal(
  forecast((a) => {
    a.snapshot.asOf = "invalid";
  }).state,
  "unknown",
);
assert.equal(
  forecast((a) => {
    a.snapshot.asOf = new Date(now + 1).toISOString();
  }).state,
  "unknown",
);
assert.equal(
  forecast((a) => {
    a.snapshot.validForSeconds = -1;
  }).state,
  "unknown",
);
for (const providerId of ["provider-b", "", null]) {
  assert.deepEqual(
    providerQuotaForecasts(
      { ...account, providerId },
      "provider-a",
      now,
      false,
    ),
    [],
  );
}
assert.deepEqual(
  providerQuotaForecasts(
    { ...account, snapshot: null },
    "provider-a",
    now,
    false,
  ),
  [],
);
assert.deepEqual(providerQuotaForecasts(account, "", now, false), []);
const mixed = structuredClone(account);
mixed.snapshot.windows.push({
  ...mixed.snapshot.windows[0],
  id: "unknown-window",
  usedPercent: null,
});
const rows = providerQuotaForecasts(mixed, "provider-a", now, false);
assert.equal(rows[0].state, "ready");
assert.equal(rows[1].state, "unknown");
assert.notEqual(rows[0].id, rows[1].id);
assert.equal(quotaPercent(0), "0%");
assert.equal(quotaPercent(null), "未知");
assert.equal(quotaPercent(7.123), "7.1%");
console.log(
  "通过：真实零与未知、独立窗口、精确账户关联、过期与读取失败、数据不足、周期边界及固定采样时间预测；输入快照不变。",
);
