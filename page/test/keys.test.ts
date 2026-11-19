// The keyboard's rule: which focused element needs keys, and when the watcher
// gives the keyboard back to the host. Events are dispatched on the test's
// jsdom document; timers are fake, so "the next task" is `vi.runAllTimers()`.

import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { needsKeys, watchKeys } from "../src/keys";
import { testKeyScope, type TestKeyScope } from "./keyScope";

/** Makes `html` the body's content and returns the element with `id`. */
function page(html: string): (id: string) => HTMLElement {
  document.body.innerHTML = html;
  return (id) => {
    const element = document.getElementById(id);
    if (!element) throw new Error(`no element #${id}`);
    return element;
  };
}

describe("needsKeys", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  test("an input that takes typing needs keys, an unknown type reading as text", () => {
    for (const type of ["text", "search", "email", "url", "tel", "password", "number", "knob"]) {
      const input = document.createElement("input");
      input.setAttribute("type", type);
      expect(needsKeys(input), type).toBe(true);
    }
  });

  test("a date or time input needs keys", () => {
    for (const type of ["date", "time", "datetime-local", "month", "week"]) {
      const input = document.createElement("input");
      input.type = type;
      expect(needsKeys(input), type).toBe(true);
    }
  });

  test("an input that takes a pointer needs none, a slider included", () => {
    for (const type of ["range", "checkbox", "radio", "button", "submit", "reset", "color", "file", "image"]) {
      const input = document.createElement("input");
      input.type = type;
      expect(needsKeys(input), type).toBe(false);
    }
  });

  test("a read-only field needs none", () => {
    const input = document.createElement("input");
    input.readOnly = true;
    const area = document.createElement("textarea");
    expect(needsKeys(area)).toBe(true);
    area.readOnly = true;
    expect(needsKeys(input)).toBe(false);
    expect(needsKeys(area)).toBe(false);
  });

  test("editable content needs keys, unless marked false", () => {
    const get = page(`
      <div id="editor" contenteditable="true"><span id="inside">x</span>
        <span id="off" contenteditable="false">y</span></div>
      <div id="plain" contenteditable="">z</div>`);
    expect(needsKeys(get("editor"))).toBe(true);
    expect(needsKeys(get("inside"))).toBe(true);
    expect(needsKeys(get("plain"))).toBe(true);
    expect(needsKeys(get("off"))).toBe(false);
  });

  test("a select needs keys", () => {
    expect(needsKeys(document.createElement("select"))).toBe(true);
  });

  test("a button, a link, a role=slider, the body and nothing need none", () => {
    const get = page(`<button id="b">b</button><a id="a" href="#">a</a><div id="s" role="slider" tabindex="0"></div>`);
    for (const id of ["b", "a", "s"]) expect(needsKeys(get(id)), id).toBe(false);
    expect(needsKeys(document.body)).toBe(false);
    expect(needsKeys(null)).toBe(false);
  });

  test("the claim holds on the element, on an ancestor and on the body", () => {
    const get = page(`<canvas id="knob" tabindex="0" data-hapto-keys></canvas>
      <div data-hapto-keys><button id="inner">i</button></div><button id="free">f</button>`);
    expect(needsKeys(get("knob"))).toBe(true);
    expect(needsKeys(get("inner"))).toBe(true);
    expect(needsKeys(get("free"))).toBe(false);
    document.body.setAttribute("data-hapto-keys", "");
    expect(needsKeys(get("free"))).toBe(true);
    document.body.removeAttribute("data-hapto-keys");
  });
});

describe("watchKeys", () => {
  let keys: TestKeyScope;
  let sent: number;
  let stop: () => void;
  let get: (id: string) => HTMLElement;

  const pointer = (type: string, target: Element) =>
    target.dispatchEvent(new PointerEvent(type, { bubbles: true, cancelable: true }));
  const click = (target: Element) => {
    pointer("pointerdown", target);
    pointer("pointerup", target);
  };
  const nextTask = () => vi.runAllTimers();

  beforeEach(() => {
    vi.useFakeTimers();
    get = page(`<div id="background"></div><label id="word">Label <input id="field" type="text"></label>
      <input id="slider" type="range"><select id="select"><option>a</option><option>b</option></select>`);
    keys = testKeyScope();
    sent = 0;
    stop = watchKeys(keys.scope, () => (sent += 1));
  });

  afterEach(() => {
    stop();
    keys.dispose();
    (document.activeElement as HTMLElement | null)?.blur();
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  test("a click on the background gives the keyboard back in the next task, not in its handler", () => {
    click(get("background"));
    expect(sent).toBe(0);
    nextTask();
    expect(sent).toBe(1);
  });

  test("a drag gives nothing back until its release, a blur during it included", () => {
    get("field").focus();
    pointer("pointerdown", get("slider"));
    get("field").blur();
    nextTask();
    expect(sent).toBe(0);
    pointer("pointerup", get("slider"));
    nextTask();
    expect(sent).toBe(1);
  });

  test("a click into a field keeps the keyboard", () => {
    pointer("pointerdown", get("field"));
    get("field").focus();
    pointer("pointerup", get("field"));
    nextTask();
    expect(sent).toBe(0);
  });

  test("a field focused after the release, as a label's click does, keeps the keyboard", () => {
    click(get("word"));
    get("field").focus();
    nextTask();
    expect(sent).toBe(0);
  });

  test("a field's blur, by Enter or Escape, gives the keyboard back", () => {
    get("field").focus();
    get("field").blur();
    nextTask();
    expect(sent).toBe(1);
  });

  test("the focus moving from a field to a slider, as Tab does, gives it back", () => {
    get("field").focus();
    get("slider").focus();
    nextTask();
    expect(sent).toBe(1);
  });

  test("nothing is given back while the page does not have the keyboard", () => {
    keys.setFocused(false);
    click(get("background"));
    get("field").focus();
    get("field").blur();
    nextTask();
    expect(sent).toBe(0);
  });

  test("a key with nothing holding the keyboard is dropped and gives it back; one in a field is the field's", () => {
    const loose = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    document.body.dispatchEvent(loose);
    expect(loose.defaultPrevented).toBe(true);
    nextTask();
    expect(sent).toBe(1);

    get("field").focus();
    const typed = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    get("field").dispatchEvent(typed);
    expect(typed.defaultPrevented).toBe(false);
    nextTask();
    expect(sent).toBe(1);
  });

  test("a release whose propagation a control stopped still gives it back", () => {
    get("background").addEventListener("pointerup", (event) => event.stopPropagation());
    click(get("background"));
    nextTask();
    expect(sent).toBe(1);
  });

  test("a select keeps the keyboard until a pick, and again once clicked", () => {
    const select = get("select");
    select.focus();
    nextTask();
    expect(sent).toBe(0);
    select.dispatchEvent(new Event("change", { bubbles: true }));
    nextTask();
    expect(sent).toBe(1);
    pointer("pointerdown", select);
    pointer("pointerup", select);
    nextTask();
    expect(sent).toBe(1);
  });

  test("events in one task give it back once", () => {
    get("field").focus();
    click(get("background"));
    get("field").blur();
    nextTask();
    expect(sent).toBe(1);
  });

  test("once stopped, it hears nothing and what it waited for is dropped", () => {
    click(get("background"));
    stop();
    nextTask();
    click(get("background"));
    nextTask();
    expect(sent).toBe(0);
  });
});
