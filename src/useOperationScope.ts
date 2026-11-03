import { useEffect, useMemo, useRef } from "react";

/** Read invalidation and one in-flight write; hiding never cancels that write. */
export function useOperationScope(active: boolean) {
  const state = useRef({ mounted: false, session: null as object | null, pending: null as object | null, generation: 0 });
  useEffect(() => {
    state.current.mounted = true;
    return () => { state.current.mounted = false; };
  }, []);
  useEffect(() => {
    state.current.session = active ? {} : null;
    return () => { state.current.session = null; state.current.generation++; };
  }, [active]);

  return useMemo(() => ({
    get pending() { return state.current.pending !== null; },
    invalidateReads() { state.current.generation++; },
    read() {
      const { session, generation } = state.current;
      return () => Boolean(session && state.current.mounted && state.current.session === session
        && state.current.generation === generation && !state.current.pending);
    },
    begin() {
      const { session } = state.current;
      if (!state.current.mounted || !session || state.current.pending) return null;
      const token = {};
      state.current.pending = token;
      state.current.generation++;
      return {
        current: () => state.current.mounted && state.current.session === session && state.current.pending === token,
        finish: () => {
          if (state.current.pending !== token) return false;
          state.current.pending = null;
          return state.current.mounted;
        },
      };
    },
  }), []);
}
