// Retire read authority before a local mutation, including an uncertain reply.
// This never retries, cancels or changes the original business operation.
let epoch = Symbol();
const listeners = new Set<() => void>();
export const capabilityAccessEpoch = () => epoch;
export function subscribeCapabilityAccess(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
export function invalidateCapabilityAccess() {
  epoch = Symbol();
  for (const listener of [...listeners]) {
    try {
      listener();
    } catch {
      console.error("local-agent: capability-access-observer");
    }
  }
}
