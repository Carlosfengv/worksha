// Backend scope changes invalidate active UI reads, never business actions.
let epoch = Symbol();
const listeners = new Set<() => void>();

export const backendReadEpoch = () => epoch;

export function subscribeBackendRecovery(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function invalidateBackendReads() {
  epoch = Symbol();
  for (const listener of [...listeners]) {
    try {
      listener();
    } catch {
      // A failed observer cannot undo a confirmed native connection or stop
      // other readers. Never log native payloads, credentials or error text.
      console.error("local-agent: backend-read-observer");
    }
  }
}

export function notifyBackendRecovery() {
  invalidateBackendReads();
}
