import { act, renderHook } from "@testing-library/react";
import { expect, test } from "vitest";

import { createClient } from "../src/client";
import { PROTOCOL_VERSION } from "../src/generated/protocol";
import { createHooks } from "../src/hooks";
import { memoryTransport } from "../src/transport";

interface Params {
  gain: unknown;
}
interface Events {
  clipWarning: { channel: number };
}
interface Commands {
  ping: { args: void; returns: null };
}
interface Values {
  label: string;
}

test("one createHooks call returns all four hooks, and they work together in one render", () => {
  const t = memoryTransport();
  const hooks = createHooks<Params, Events, Commands, Values>(createClient(t.page));
  expect(typeof hooks.useParam).toBe("function");
  expect(typeof hooks.useValue).toBe("function");
  expect(typeof hooks.useEvent).toBe("function");
  expect(typeof hooks.useCommand).toBe("function");

  const clips: number[] = [];
  const { result } = renderHook(() => {
    hooks.useEvent("clipWarning", (p) => clips.push(p.channel));
    return { gain: hooks.useParam("gain"), label: hooks.useValue("label"), ping: hooks.useCommand("ping") };
  });
  act(() => {
    t.native.send({
      type: "snapshot",
      protocol: PROTOCOL_VERSION,
      params: [{ param: "gain", normalized: 0.6, text: "x" }],
      values: [{ key: "label", value: "Lead" }],
    });
    t.native.send({ type: "event", name: "clipWarning", payload: { channel: 1 } });
  });
  expect(result.current.gain.value).toBe(0.6);
  expect(result.current.label.value).toBe("Lead");
  expect(clips).toEqual([1]);
  expect(result.current.ping.pending).toBe(false);
});
