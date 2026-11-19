// How messages travel between the page and the native side.
//
// The page never knows what carries them: whatever does is a Transport, and
// nothing else in this package imports one. Two exist: the in-memory
// transport, for tests (a mock native side in a browser could use it too, but
// none is built), and the WebView transport, for a page in a plugin's window
// (`hapto-webview`). The transport for a standalone app is planned.

import type { NativeMessage, PageMessage } from "./generated/protocol";
import { HAND_BACK, type KeyScope, watchKeys } from "./keys";

/** Carries messages between the page and the native side, in the order they
    were sent, both ways: the client's order and gesture guarantees rest on it. */
export interface Transport {
  /** Sends one message to the native side. Never throws; a message the
      native side cannot decode is its problem to report. */
  send(message: PageMessage): void;
  /** Calls `handler` for every message from the native side until the
      returned function is called. */
  listen(handler: (message: NativeMessage) => void): () => void;
}

/** Both ends of an in-memory transport: `page` is what the page's client
    uses, `native` stands in for the native side. Delivery is synchronous,
    in send order. */
export interface MemoryTransport {
  page: Transport;
  native: {
    /** Delivers a message to the page's listeners. */
    send(message: NativeMessage): void;
    /** Every message the page has sent, oldest first. */
    readonly received: PageMessage[];
    /** Calls `handler` for each message the page sends from now on. */
    listen(handler: (message: PageMessage) => void): () => void;
  };
}

/** An in-memory transport, for tests and for a mock native side. */
export function memoryTransport(): MemoryTransport {
  const pageListeners = new Set<(message: NativeMessage) => void>();
  const nativeListeners = new Set<(message: PageMessage) => void>();
  const received: PageMessage[] = [];
  return {
    page: {
      send(message) {
        received.push(message);
        for (const listener of [...nativeListeners]) listener(message);
      },
      listen(handler) {
        pageListeners.add(handler);
        return () => pageListeners.delete(handler);
      },
    },
    native: {
      send(message) {
        for (const listener of [...pageListeners]) listener(message);
      },
      received,
      listen(handler) {
        nativeListeners.add(handler);
        return () => nativeListeners.delete(handler);
      },
    },
  };
}

/** What a page in a plugin's window finds on `window`: `ipc`, which the
    WebView provides to post to the native side, `__hapto`, which
    `webviewTransport` installs for the native side to deliver through, and
    the document, the window's events and its timers, which the keyboard's
    watcher reads. */
export interface WebViewGlobals extends Partial<KeyScope> {
  ipc?: { postMessage(text: string): void };
  __hapto?: { receive(text: string): void };
}

/** The transport of a page in a plugin's window, whose native side is
    `hapto-webview`: each message goes out as JSON text through
    `window.ipc.postMessage`, and the native side delivers its own, in
    order, as JSON text to `window.__hapto.receive`, which this installs.

    `scope` is where the two live, `globalThis` in a page; a test passes a
    fake. Outside a plugin's window, with no `ipc`, sends are dropped. A
    native message that is not JSON is dropped too: the client checks the
    shape of every message that is.

    In a plugin's window, with `ipc` and a document, it also gives the
    keyboard back to the host whenever nothing in the page needs keys
    (`watchKeys`). The hand-back goes straight through `ipc`, never through
    the client, so it is never held for the snapshot. */
export function webviewTransport(scope: WebViewGlobals = globalThis as WebViewGlobals): Transport {
  const listeners = new Set<(message: NativeMessage) => void>();
  if (scope.ipc && isKeyScope(scope)) watchKeys(scope, () => scope.ipc?.postMessage(HAND_BACK));
  scope.__hapto = {
    receive(text) {
      let message: NativeMessage;
      try {
        message = JSON.parse(text) as NativeMessage;
      } catch {
        return;
      }
      for (const listener of [...listeners]) listener(message);
    },
  };
  return {
    send(message) {
      scope.ipc?.postMessage(JSON.stringify(message));
    },
    listen(handler) {
      listeners.add(handler);
      return () => listeners.delete(handler);
    },
  };
}

/** Whether `scope` carries all the keyboard's watcher reads: a window does,
    a test's plain object may not. */
function isKeyScope(scope: WebViewGlobals): scope is WebViewGlobals & KeyScope {
  return (
    scope.document !== undefined &&
    typeof scope.addEventListener === "function" &&
    typeof scope.removeEventListener === "function" &&
    typeof scope.setTimeout === "function" &&
    typeof scope.clearTimeout === "function"
  );
}
