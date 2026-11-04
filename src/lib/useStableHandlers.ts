import { useMemo, useRef } from "react";

/* eslint-disable @typescript-eslint/no-explicit-any */
type Handlers = Record<string, (...args: any[]) => any>;

/** The same handler functions on every render, each calling the latest
 *  version passed in. Lets memoized children skip re-renders without
 *  holding stale closures. The set of keys must not change. */
export function useStableHandlers<T extends Handlers>(handlers: T): T {
  const latest = useRef(handlers);
  latest.current = handlers;
  return useMemo(
    () => Object.fromEntries(Object.keys(handlers).map((k) => [k, (...args: any[]) => latest.current[k](...args)])) as T,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
}
