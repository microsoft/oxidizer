// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

export function rate(current, previous, elapsedMs) {
  if (elapsedMs <= 0) return null;
  const delta = BigInt(current ?? 0) - BigInt(previous ?? 0);
  return delta < 0n ? null : Math.round(Number(delta) * 1000 / elapsedMs);
}

export function sample(data, previous, now) {
  const past = previous.error ? undefined : previous.data;
  const sameSession = past && (data.sessionId ?? "0") === (past.sessionId ?? "0");
  if (!sameSession || !data.classEvents || !past.classEvents) {
    return { data, time: now, history: [] };
  }
  const elapsed = now - previous.time;
  const rates = data.classEvents.map((value, index) => rate(value, past.classEvents[index], elapsed));
  return {
    data, past, time: now, pastTime: previous.time,
    history: [...(previous.history ?? []).slice(-119), { time: now, rates }],
  };
}
