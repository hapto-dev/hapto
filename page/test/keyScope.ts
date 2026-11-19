// A KeyScope over the test's jsdom window, with a fake `hasFocus`: jsdom's is
// true only while an element has the focus, where a WebView's follows the
// keyboard. Its listeners are removed by `dispose`, so a watcher a transport
// installed and never stops does not reach into the next test.

import type { KeyScope } from "../src/keys";

export interface TestKeyScope {
  scope: KeyScope;
  /** Sets what `document.hasFocus()` answers: whether the page has the
      keyboard. */
  setFocused(focused: boolean): void;
  /** Removes every listener added through `scope`. */
  dispose(): void;
}

export function testKeyScope(): TestKeyScope {
  let focused = true;
  const added: [string, (event: Event) => void, boolean][] = [];
  const scope: KeyScope = {
    document: {
      get activeElement() {
        return document.activeElement;
      },
      hasFocus: () => focused,
    },
    addEventListener(type, listener, capture) {
      added.push([type, listener, capture]);
      window.addEventListener(type, listener, capture);
    },
    removeEventListener(type, listener, capture) {
      window.removeEventListener(type, listener, capture);
    },
    // Read from `window` at each call, so fake timers installed after this
    // scope was made still apply.
    setTimeout: (handler, timeout) => window.setTimeout(handler, timeout),
    clearTimeout: (id) => window.clearTimeout(id),
  };
  return {
    scope,
    setFocused(next) {
      focused = next;
    },
    dispose() {
      for (const [type, listener, capture] of added) window.removeEventListener(type, listener, capture);
      added.length = 0;
    },
  };
}
