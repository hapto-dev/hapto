import { act, renderHook } from "@testing-library/react";
import { describe, expect, test } from "vitest";

import { createClient } from "../src/client";
import { PROTOCOL_VERSION, type NativeMessage } from "../src/generated/protocol";
import { createHooks } from "../src/hooks";
import { memoryTransport, type MemoryTransport } from "../src/transport";

interface Values {
  label: string;
  sample: string | null;
}

function setup() {
  const t = memoryTransport();
  const client = createClient(t.page);
  const { useValue } = createHooks<Record<string, never>, Record<string, never>, Record<string, never>, Values>(
    client,
  );
  const writes = () => t.native.received.slice(1);
  return { t, client, useValue, writes };
}

const snapshot = (label: string): NativeMessage => ({
  type: "snapshot",
  protocol: PROTOCOL_VERSION,
  params: [],
  values: [{ key: "label", value: label }],
});

const label = (value: unknown): NativeMessage => ({ type: "value", state: { key: "label", value } });

// A native side that answers every write with what it holds: the write when
// `stores` says so, and otherwise what it held before.
function answer(t: MemoryTransport, stores: (value: unknown) => boolean) {
  let held: unknown = "Untitled";
  t.native.listen((m) => {
    if (m.type !== "setValue") return;
    if (stores(m.value)) held = m.value;
    t.native.send({ type: "value", state: { key: m.key, value: held } });
  });
}

describe("useValue", () => {
  test("is undefined until the snapshot arrives, then shows its value", () => {
    const { t, useValue } = setup();
    const { result } = renderHook(() => useValue("label"));
    expect(result.current.value).toBeUndefined();
    act(() => t.native.send(snapshot("Lead")));
    expect(result.current.value).toBe("Lead");
  });

  test("seeds from the client's cache on mount", () => {
    const { t, useValue } = setup();
    t.native.send(snapshot("Lead"));
    const { result } = renderHook(() => useValue("label"));
    expect(result.current.value).toBe("Lead");
  });

  test("shows a native change at once, whatever made it", () => {
    const { t, useValue } = setup();
    t.native.send(snapshot("Lead"));
    const { result } = renderHook(() => useValue("label"));
    act(() => t.native.send(label("Loaded")));
    expect(result.current.value).toBe("Loaded");
    // A parameter of the same name is not the value.
    act(() => t.native.send({ type: "param", state: { param: "label", normalized: 0.5, text: "x" } }));
    expect(result.current.value).toBe("Loaded");
  });

  test("set sends the write and shows it until the answer, which it then shows", () => {
    const { t, useValue, writes } = setup();
    t.native.send(snapshot("Lead"));
    const { result } = renderHook(() => useValue("label"));
    act(() => result.current.set("Pad"));
    expect(writes()).toEqual([{ type: "setValue", key: "label", value: "Pad" }]);
    expect(result.current.value).toBe("Pad");
    act(() => t.native.send(label("Pad")));
    expect(result.current.value).toBe("Pad");
  });

  test("a write the native side did not store goes back to what it holds", () => {
    const { t, useValue } = setup();
    answer(t, (value) => typeof value === "string" && value !== "");
    t.native.send(snapshot("Untitled"));
    const { result } = renderHook(() => useValue("label"));
    act(() => result.current.set(""));
    expect(result.current.value).toBe("Untitled");
    act(() => result.current.set("Lead"));
    expect(result.current.value).toBe("Lead");
  });

  test("a write before the snapshot waits for it, and the answer ends on the store", () => {
    const { t, useValue } = setup();
    answer(t, () => true);
    const { result } = renderHook(() => useValue("label"));
    act(() => result.current.set("Pad"));
    expect(t.native.received).toEqual([{ type: "ready", protocol: PROTOCOL_VERSION }]);
    act(() => t.native.send(snapshot("Lead")));
    expect(t.native.received.slice(1)).toEqual([{ type: "setValue", key: "label", value: "Pad" }]);
    expect(result.current.value).toBe("Pad");
  });

  test("a null value is a value, shown as null", () => {
    const { t, useValue } = setup();
    t.native.send({ type: "snapshot", protocol: PROTOCOL_VERSION, params: [], values: [{ key: "sample", value: null }] });
    const { result } = renderHook(() => useValue("sample"));
    expect(result.current.value).toBeNull();
  });

  test("a new key binds the new value and stops hearing the old", () => {
    const { t, useValue } = setup();
    t.native.send({
      type: "snapshot",
      protocol: PROTOCOL_VERSION,
      params: [],
      values: [
        { key: "label", value: "Lead" },
        { key: "sample", value: "kick.wav" },
      ],
    });
    const { result, rerender } = renderHook(({ key }: { key: "label" | "sample" }) => useValue(key), {
      initialProps: { key: "label" as "label" | "sample" },
    });
    expect(result.current.value).toBe("Lead");
    rerender({ key: "sample" });
    expect(result.current.value).toBe("kick.wav");
    act(() => t.native.send(label("Pad")));
    expect(result.current.value).toBe("kick.wav");
  });

  test("a failed client shows its failure and drops the write", () => {
    const { t, useValue, writes } = setup();
    const { result } = renderHook(() => useValue("label"));
    act(() => t.native.send({ type: "reject", protocol: PROTOCOL_VERSION + 1, message: "too new" }));
    expect(result.current.failure?.message).toContain("protocol mismatch");
    act(() => result.current.set("Pad"));
    expect(writes()).toEqual([]);
  });
});
