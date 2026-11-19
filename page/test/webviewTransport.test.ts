import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import shell from "../../crates/webview/tests/fixtures/shell.json";
import { createClient } from "../src/client";
import { PROTOCOL_VERSION, type NativeMessage } from "../src/generated/protocol";
import { HAND_BACK } from "../src/keys";
import { memoryTransport, webviewTransport, type WebViewGlobals } from "../src/transport";
import { testKeyScope, type TestKeyScope } from "./keyScope";

// A fake of what a plugin window's WebView gives the page: `ipc`, which keeps
// what the page posts, and the `__hapto` the transport installs, which the
// native side delivers through.
function window(): { scope: WebViewGlobals; posted: string[]; deliver: (message: NativeMessage) => void } {
  const posted: string[] = [];
  const scope: WebViewGlobals = { ipc: { postMessage: (text) => posted.push(text) } };
  const deliver = (message: NativeMessage) => scope.__hapto?.receive(JSON.stringify(message));
  return { scope, posted, deliver };
}

describe("webviewTransport", () => {
  test("sends each message as JSON text through ipc", () => {
    const { scope, posted } = window();
    const transport = webviewTransport(scope);
    transport.send({ type: "ready", protocol: PROTOCOL_VERSION });
    transport.send({ type: "set", param: "gain", normalized: 0.5 });
    expect(posted.map((text) => JSON.parse(text))).toEqual([
      { type: "ready", protocol: PROTOCOL_VERSION },
      { type: "set", param: "gain", normalized: 0.5 },
    ]);
  });

  test("delivers what the native side posts to every listener, in order", () => {
    const { scope, deliver } = window();
    const transport = webviewTransport(scope);
    const first: NativeMessage[] = [];
    const second: NativeMessage[] = [];
    transport.listen((message) => first.push(message));
    const stop = transport.listen((message) => second.push(message));
    const param = (normalized: number): NativeMessage => ({
      type: "param",
      state: { param: "gain", normalized, text: `${normalized}` },
    });
    deliver(param(0.25));
    stop();
    deliver(param(0.5));
    expect(first).toEqual([param(0.25), param(0.5)]);
    expect(second).toEqual([param(0.25)]);
  });

  test("drops native text that is not JSON, and goes on", () => {
    const { scope, deliver } = window();
    const transport = webviewTransport(scope);
    const got: NativeMessage[] = [];
    transport.listen((message) => got.push(message));
    scope.__hapto?.receive("not json");
    deliver({ type: "event", name: "learned", payload: null });
    expect(got).toEqual([{ type: "event", name: "learned", payload: null }]);
  });

  test("drops sends outside a plugin window, where there is no ipc", () => {
    const scope: WebViewGlobals = {};
    const transport = webviewTransport(scope);
    expect(() => transport.send({ type: "ready", protocol: PROTOCOL_VERSION })).not.toThrow();
  });

  test("carries a client's handshake both ways", () => {
    const { scope, posted, deliver } = window();
    const client = createClient(webviewTransport(scope));
    expect(JSON.parse(posted[0] ?? "")).toEqual({ type: "ready", protocol: PROTOCOL_VERSION });
    deliver({
      type: "snapshot",
      protocol: PROTOCOL_VERSION,
      params: [{ param: "gain", normalized: 0.5, text: "-24 dB" }],
      values: [],
    });
    const gain = client.bindParam("gain", () => {});
    expect(gain.current()?.text).toBe("-24 dB");
    gain.set(0.75);
    expect(JSON.parse(posted.at(-1) ?? "")).toEqual({ type: "set", param: "gain", normalized: 0.75 });
    client.dispose();
  });
});

describe("webviewTransport and the keyboard", () => {
  let keys: TestKeyScope;

  const click = () => {
    for (const type of ["pointerdown", "pointerup"]) {
      document.body.dispatchEvent(new PointerEvent(type, { bubbles: true }));
    }
    // The next task, and not the client's resend of `ready` a second later.
    vi.advanceTimersByTime(1);
  };

  beforeEach(() => {
    vi.useFakeTimers();
    keys = testKeyScope();
  });

  afterEach(() => {
    keys.dispose();
    vi.useRealTimers();
  });

  test("the hand-back is the shell fixture's text", () => {
    expect(JSON.stringify(shell)).toBe(HAND_BACK);
  });

  test("gives the keyboard back through ipc, before the snapshot too, never held by the client", () => {
    const posted: string[] = [];
    const scope: WebViewGlobals = { ...keys.scope, ipc: { postMessage: (text) => posted.push(text) } };
    const client = createClient(webviewTransport(scope));
    client.bindParam("gain", () => {}).set(0.5);
    click();
    expect(posted).toEqual([JSON.stringify({ type: "ready", protocol: PROTOCOL_VERSION }), HAND_BACK]);
    scope.__hapto?.receive(
      JSON.stringify({ type: "snapshot", protocol: PROTOCOL_VERSION, params: [], values: [] } satisfies NativeMessage),
    );
    expect(posted.filter((text) => text === HAND_BACK)).toHaveLength(1);
    client.dispose();
  });

  test("watches nothing outside a plugin's window, with no ipc", () => {
    const scope: WebViewGlobals = { ...keys.scope };
    webviewTransport(scope);
    const key = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    document.body.dispatchEvent(key);
    expect(key.defaultPrevented).toBe(false);
  });

  test("the memory transport gives nothing back", () => {
    const t = memoryTransport();
    const client = createClient(t.page);
    click();
    const key = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    document.body.dispatchEvent(key);
    expect(key.defaultPrevented).toBe(false);
    expect(t.native.received).toEqual([{ type: "ready", protocol: PROTOCOL_VERSION }]);
    client.dispose();
  });
});
