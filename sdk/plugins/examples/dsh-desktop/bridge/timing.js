// DSH 0.2.2-rc.1's timed assistant records preserve delta boundaries.
// A delta qualifies as output, but its length does not measure token usage.
const counter = (value) => Number.isSafeInteger(value) && value >= 0;
const fragment = (value) => typeof value === "string" && value !== "";
const sameStep = (step, data) => step && step.turn === data.turn && step.step === data.step;

export function isTokenDelta(chunk) {
  if (chunk?.type === "text-delta" || chunk?.type === "reasoning-delta")
    return fragment(chunk.text);
  return chunk?.type === "tool-call-delta" &&
    (fragment(chunk.argumentsDelta) || fragment(chunk.name));
}

function firstToken(stream) {
  for (const record of Array.isArray(stream) ? stream : []) {
    if (record.type === "chunk") {
      if (counter(record.time) && isTokenDelta(record.chunk)) return record.time;
      continue;
    }
    const tool = record.type === "tool-call-chunks";
    if (!tool && !["text-chunks", "reasoning-chunks"].includes(record.type)) continue;
    const parts = tool ? record.args : record.texts;
    let time = record.time0;
    if (!counter(time) || !Array.isArray(parts)) continue;
    if (tool && fragment(record.name)) return time;
    for (let index = 0; index < parts.length; index++) {
      if (index > 0) {
        const gap = record.dt?.[index - 1];
        if (!counter(gap)) break;
        time += gap;
        if (!counter(time)) break;
      }
      if (fragment(parts[index])) return time;
    }
  }
  return null;
}

export function taskTiming(events, messageNativeId, live) {
  const input = events.find((event) => String(event.seq) === messageNativeId);
  if (!input || !counter(input.seq) || !counter(input.time)) return null;
  let step = null;
  let taskTurn = null, completedAt = null;
  let firstTokenMs = null, model = null;
  let outputTokens = 0, knownOutput = false;
  let completedDecodeMs = 0, completedTokens = 0;
  for (const event of events) {
    const data = event.data ?? {};
    if (event.type === "step/start") {
      step = counter(event.time) && counter(data.turn) && counter(data.step)
        ? { turn: data.turn, step: data.step, startedAt: event.time, first: null }
        : null;
      if (event.seq > input.seq && step?.startedAt >= input.time)
        taskTurn ??= step.turn;
    } else if (event.type === "turn/end") {
      if (event.seq > input.seq && taskTurn !== null && data.turn === taskTurn) {
        if (counter(event.time) && event.time >= input.time) completedAt = event.time;
        break;
      }
      if (step?.turn === data.turn) step = null;
    } else if (event.type === "step/end" && sameStep(step, data)) {
      step = null;
    } else if (event.seq > input.seq && sameStep(step, data) &&
        (taskTurn === null || data.turn === taskTurn) &&
        (event.type === "assistant/message" || event.type === "assistant/attempt")) {
      taskTurn ??= data.turn;
      const first = firstToken(data.stream);
      if (counter(first) && first >= Math.max(input.time, step.startedAt) && first <= event.time) {
        step.first ??= first;
        firstTokenMs ??= first - step.startedAt;
      }
      if (event.type === "assistant/message") {
        const reported = data.usage?.outputTokens;
        if (counter(reported)) {
          outputTokens += reported;
          knownOutput = true;
          // DSH's session stats use first output through the assembled response.
          if (!data.interrupted && step.first !== null && counter(event.time) && event.time > step.first) {
            completedDecodeMs += event.time - step.first;
            completedTokens += reported;
          }
        }
        if (typeof data.message?.source?.model === "string")
          model = data.message.source.model.slice(0, 200);
        step = null;
      }
    }
  }

  let runningTokens = null, runningDecodeMs = null;
  if (completedAt === null && live && sameStep(step, live) &&
      (taskTurn === null || live.turn === taskTurn) && live.startedAfterSeq >= input.seq &&
      counter(live.first) && live.first >= Math.max(input.time, step.startedAt)) {
    firstTokenMs ??= live.first - step.startedAt;
    if (counter(live.usage?.tokens) && counter(live.usage.at) && live.usage.at > live.first) {
      runningTokens = String(live.usage.tokens);
      runningDecodeMs = live.usage.at - live.first;
    }
  }
  return {
    messageNativeId, messageStartedAtMs: input.time, model, firstTokenMs,
    outputTokens: knownOutput && counter(outputTokens) ? String(outputTokens) : null,
    completedTokens: completedDecodeMs && counter(completedTokens) ? String(completedTokens) : null,
    completedDecodeMs: counter(completedDecodeMs) && completedDecodeMs > 0 ? completedDecodeMs : null,
    runningTokens, runningDecodeMs,
    durationMs: completedAt === null ? null : completedAt - input.time,
  };
}
