import { createModuleGraph } from "./jsx-tree.mjs";

export const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

// 执行真实 controller；仅替换 React 调度和外部请求，依赖变化执行清理并重新挂载 effect。
export function createControllerHarness(url, name, stubs = {}) {
  const hooks = [];
  let cursor = 0, changed = false, effects = [], args = [], value;
  let unmounted = false, writesAfterUnmount = 0;
  const same = (left, right) => left && right && left.length === right.length && left.every((item, index) => Object.is(item, right[index]));
  const react = {
    memo: component => component,
    useState(initial) {
      const index = cursor++;
      if (!(index in hooks)) hooks[index] = { value: typeof initial === "function" ? initial() : initial };
      return [hooks[index].value, (update) => {
        if (unmounted) { writesAfterUnmount++; return; }
        const next = typeof update === "function" ? update(hooks[index].value) : update;
        if (!Object.is(next, hooks[index].value)) { hooks[index].value = next; changed = true; }
      }];
    },
    useRef(initial) {
      const index = cursor++;
      if (!(index in hooks)) hooks[index] = { current: initial };
      return hooks[index];
    },
    useMemo(factory, deps) {
      const index = cursor++;
      if (!same(hooks[index]?.deps, deps)) hooks[index] = { deps, value: factory() };
      return hooks[index].value;
    },
    useCallback(callback, deps) { return react.useMemo(() => callback, deps); },
    useDeferredValue(value) { return value; },
    useId() { return react.useRef(":controller:").current; },
    useEffect(effect, deps) {
      const index = cursor++;
      if (same(hooks[index]?.deps, deps)) return;
      effects.push(() => { hooks[index]?.cleanup?.(); hooks[index] = { deps, cleanup: effect() }; });
    },
    useLayoutEffect(effect, deps) { react.useEffect(effect, deps); },
  };
  const graph = createModuleGraph(url, { stubs: { ...stubs, react } });
  const render = (...nextArgs) => {
    if (nextArgs.length) args = nextArgs;
    for (let attempt = 0; attempt < 30; attempt++) {
      cursor = 0;
      changed = false;
      value = graph.exports[name](...args);
      effects.splice(0).forEach((effect) => effect());
      if (!changed) return value;
    }
    throw new Error("controller 未能稳定，可能出现 effect 循环");
  };
  return {
    render,
    async flush() {
      for (let index = 0; index < 30; index++) { await Promise.resolve(); if (changed && !unmounted) render(); }
      return value;
    },
    unmount() { unmounted = true; hooks.forEach((hook) => hook?.cleanup?.()); },
    get writesAfterUnmount() { return writesAfterUnmount; },
  };
}

export function installControllerWindow(t) {
  const previous = globalThis.window;
  const events = new EventTarget();
  const timers = new Map();
  let nextId = 0;
  globalThis.window = Object.assign(events, {
    setTimeout(callback) { const id = ++nextId; timers.set(id, callback); return id; },
    clearTimeout(id) { timers.delete(id); },
  });
  t.after(() => { if (previous === undefined) delete globalThis.window; else globalThis.window = previous; });
  return {
    timers,
    tick() { const pending = [...timers.values()]; timers.clear(); pending.forEach((callback) => callback()); },
    dispatch(name) { events.dispatchEvent(new Event(name)); },
  };
}
