import type { GatewayActivity, NativeActivity } from "./types";

export interface MessageGroup {
  id: string;
  sessionId: string | null;
  startedAt: string | null;
  uncertain: boolean;
  gateways: GatewayActivity[];
  natives: NativeActivity[];
}
export function nativeMessageTime(activity: NativeActivity) {
  const value =
    activity.messageStartedAt === undefined
      ? activity.userInput?.at
      : activity.messageStartedAt;
  return value && Number.isFinite(Date.parse(value)) ? value : null;
}
const nativeKey = (a: NativeActivity) =>
  `message:${a.sessionId}:${a.messageId ?? nativeMessageTime(a) ?? "unknown"}`;

/** A continuation never resets the already measured first request of a message. */
export function messageFirstRequest(gateways: GatewayActivity[]) {
  const first = gateways.reduce<GatewayActivity | undefined>(
    (first, current) => {
      if (!first) return current;
      const order =
        Date.parse(current.request.startedAt) -
        Date.parse(first.request.startedAt);
      return order < 0 || (order === 0 && current.request.id < first.request.id)
        ? current
        : first;
    },
    undefined,
  );
  const started = first?.request.activityMessage?.startedAt;
  // The bounded history may no longer contain the actual first request.
  return started && Date.parse(started) < Date.parse(first!.request.startedAt)
    ? undefined
    : first;
}

/** Exact Sessions and message identities; previews and titles are never keys. */
export function messageGroups(
  gateways: GatewayActivity[],
  natives: NativeActivity[],
) {
  const groups = new Map<string, MessageGroup>();
  const sessions = new Map<string, NativeActivity[]>();
  const get = (
    id: string,
    sessionId: string | null,
    startedAt: string | null,
    uncertain: boolean,
  ) => {
    let group = groups.get(id);
    if (!group) {
      group = {
        id,
        sessionId,
        startedAt,
        uncertain,
        gateways: [],
        natives: [],
      };
      groups.set(id, group);
    } else if (!group.startedAt && startedAt) {
      group.startedAt = startedAt;
    }
    return group;
  };
  for (const activity of natives) {
    const current = sessions.get(activity.sessionId) ?? [];
    current.push(activity);
    sessions.set(activity.sessionId, current);
    get(
      nativeKey(activity),
      activity.sessionId,
      nativeMessageTime(activity),
      !activity.messageId && !nativeMessageTime(activity),
    ).natives.push(activity);
  }
  for (const values of sessions.values()) {
    values.sort(
      (a, b) =>
        (Date.parse(nativeMessageTime(b) ?? "") || 0) -
        (Date.parse(nativeMessageTime(a) ?? "") || 0),
    );
  }
  // Resolve a persisted gateway message once using its first request, so a
  // late tool continuation cannot be reassigned to a later user message.
  const aliases = new Map<string, NativeActivity>();
  for (const activity of gateways) {
    const { request } = activity;
    const meta = request.activityMessage;
    if (!request.sessionId || !meta) continue;
    const candidates = sessions.get(request.sessionId);
    const native =
      candidates?.find((a) => a.messageId === meta.id) ??
      candidates?.find((a) => {
        const time = nativeMessageTime(a);
        return time && Date.parse(time) <= Date.parse(meta.startedAt);
      });
    if (native) aliases.set(`${request.sessionId}:${meta.id}`, native);
  }
  for (const activity of gateways) {
    const { request } = activity;
    const session = request.sessionId;
    const meta = request.activityMessage;
    const native = session
      ? meta
        ? aliases.get(`${session}:${meta.id}`)
        : sessions.get(session)?.find((a) => {
            const time = nativeMessageTime(a);
            return time && Date.parse(time) <= Date.parse(request.startedAt);
          })
      : undefined;
    const id = native
      ? nativeKey(native)
      : session && meta
        ? `message:${session}:${meta.id}`
        : `gateway:${request.id}`;
    const startedAt = native
      ? (nativeMessageTime(native) ?? meta?.startedAt ?? null)
      : (meta?.startedAt ?? null);
    get(id, session, startedAt, !native && !meta).gateways.push(activity);
  }
  return [...groups.values()];
}
