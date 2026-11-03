import assert from "node:assert/strict";
import { activityMetrics, relativeTime, timingNote } from "../src/features/observation/display.ts";

const request = {
  phase: "streaming",
  textTokensPerSecond: 25,
  averageTextTokensPerSecond: null,
  throughput: 8,
  stale: false,
  request: { status: "success", ttftMs: 1490, timingIssue: null },
};
const native = { stale: true, turnTtftMs: null };
let view = activityMetrics({ latest: request, native, stale: false });
assert.equal(view.ttft.value, "1.49");
assert.equal(view.ttft.label, "最近请求 TTFT");
assert.equal(view.speed.value, "8.0");
assert.equal(view.speed.label, "最近请求吞吐");
view = activityMetrics({
  latest: { ...request, averageTextTokensPerSecond: 20 },
  native,
  stale: true,
});
assert.equal(view.speed.label, "最近文本均速 · 估算");
assert.equal(view.speed.value, "20.0");
assert.equal(view.ttft.value, "1.49");
view = activityMetrics({ active: request, native, stale: false });
assert.equal(view.speed.label, "当前文本速度");
assert.equal(view.speed.value, "25.0");
view = activityMetrics({
  active: { ...request, textTokensPerSecond: 0 },
  stale: false,
});
assert.equal(view.speed.value, "0.0");
view = activityMetrics({
  active: { ...request, textTokensPerSecond: null, phase: "tool_arguments" },
  stale: false,
});
assert.equal(view.speed.value, "生成工具参数");
assert.equal(view.speed.unit, "");
view = activityMetrics({
  active: {
    ...request,
    textTokensPerSecond: null,
    phase: "waiting_first",
    request: { ttftMs: null, timingIssue: null },
  },
  stale: false,
});
assert.equal(view.ttft.value, "等待首出");
view = activityMetrics({ active: request, native, stale: true });
assert.equal(view.speed.value, "等待更新");
assert.equal(view.ttft.value, "1.49");
view = activityMetrics({ native, stale: false });
assert.equal(view.speed.value, "未采集");
assert.equal(view.ttft.value, "未采集");
assert.equal(view.ttft.unit, "");
view = activityMetrics({ native: { turnTtftMs: 370 }, stale: true });
assert.equal(view.ttft.value, "0.370");
view = activityMetrics({
  latest: {
    ...request,
    throughput: null,
    request: { ttftMs: null, timingIssue: "非流式响应", status: "success" },
  },
  stale: false,
});
assert.equal(view.ttft.value, "首出未记录");
assert.equal(view.speed.value, "未记录");
view = activityMetrics({ active: { ...request, textTokensPerSecond: null,
  request: { ttftMs: 1490, timingIssue: "部分流事件无法解析" } }, stale: false });
assert.equal(view.ttft.value, "1.49");
assert.equal(view.speed.value, "采样恢复中");
view = activityMetrics({ active: { ...request,
  request: { ttftMs: 1490, timingIssue: "部分流事件无法解析" } }, stale: false });
assert.equal(view.speed.value, "25.0");
view = activityMetrics({ active: { ...request, phase: "tool_arguments" }, stale: false });
assert.equal(view.speed.value, "生成工具参数");
const now = Date.parse("2026-10-24T02:00:00Z");
for (const [ms, expected] of [[0,"0 秒前"],[59000,"59 秒前"],[60000,"1 分钟前"],[3599000,"59 分钟前"],[3600000,"1 小时前"],[7200000,"2 小时前"],[-1000,"0 秒前"]]) {
  assert.equal(relativeTime(new Date(now-ms).toISOString(),now), expected);
}
assert.equal(relativeTime("bad", now), "时间未知");
assert.equal(timingNote("非流式响应"), "非流式响应未提供时序");
console.log(
  "通过：实时、完成保留、旧记录降级、原生缺失、工具参数、零速度、非流式与过期状态；原生活动不覆盖网关计时。",
);
