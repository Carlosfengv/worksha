import { useCallback, useEffect, useState } from "react";
import {
  LOCAL_TOKEN_CANCELLED,
  localTokenIdentity,
  sameLocalTokenIdentity,
  type LocalTokenIdentity,
} from "../local-token-operation";

export function useLocalTokenError() {
  const [failure, setFailure] = useState<{
    message: string;
    identity?: LocalTokenIdentity;
  } | null>(null);
  const setError = useCallback(
    (message: string | null, identity?: LocalTokenIdentity) => {
      setFailure(message === null ? null : { message, identity });
    },
    [],
  );
  useEffect(() => {
    const cancelled = (event: Event) => {
      if (!(event instanceof CustomEvent)) return;
      let identity: LocalTokenIdentity;
      try {
        identity = localTokenIdentity(event.detail);
      } catch {
        return;
      }
      setFailure((current) =>
        current?.identity && sameLocalTokenIdentity(current.identity, identity)
          ? null
          : current,
      );
    };
    window.addEventListener(LOCAL_TOKEN_CANCELLED, cancelled);
    return () => window.removeEventListener(LOCAL_TOKEN_CANCELLED, cancelled);
  }, []);
  return [failure?.message ?? null, setError] as const;
}
