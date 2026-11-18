// The gain's page: a slider on the gain, which drags it as a user would, a
// field for the plugin's label, a value the plugin binds, and the plugin's
// version, through a command.

import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { createClient, createHooks, webviewTransport } from "@hapto/page";

import type { Params } from "./generated/params";
import "./styles.css";

/** The commands `examples/gain/src/page.rs` adds. */
interface Commands {
  version: { args: void; returns: string };
}

/** The values `examples/gain/src/page.rs` binds. */
interface Values {
  label: string;
}

const client = createClient(webviewTransport());

// A stand-in for the keyboard watcher `@hapto/page` will install, to be
// replaced by it: after a click, a drag or a blur, in the next task, the
// keyboard goes back to the host unless the label field has it.
let pointerDown = false;
let asked = false;
const ask = () => {
  if (asked) return;
  asked = true;
  setTimeout(() => {
    asked = false;
    const field = document.activeElement?.matches('input[type="text"]') ?? false;
    if (document.hasFocus() && !pointerDown && !field) {
      (window as { ipc?: { postMessage(text: string): void } }).ipc?.postMessage('{"shell":"releaseKeys"}');
    }
  }, 0);
};
window.addEventListener("pointerdown", () => (pointerDown = true), true);
for (const type of ["pointerup", "pointercancel"]) {
  window.addEventListener(type, () => ((pointerDown = false), ask()), true);
}
window.addEventListener("focusout", ask, true);

/** Who has the keyboard, as `document.hasFocus()` says: part of the stand-in. */
function KeyboardLine() {
  const [page, setPage] = useState(document.hasFocus());
  useEffect(() => {
    const read = () => setTimeout(() => setPage(document.hasFocus()), 0);
    window.addEventListener("focus", read);
    window.addEventListener("blur", read);
    return () => {
      window.removeEventListener("focus", read);
      window.removeEventListener("blur", read);
    };
  }, []);
  return <p className="version">keyboard: {page ? "page" : "host"}</p>;
}
const { useParam, useValue, useCommand } = createHooks<Params, Record<string, never>, Commands, Values>(client);

function GainSlider() {
  const gain = useParam("gain");
  return (
    <label className="row">
      <span className="name">Gain</span>
      <input
        type="range"
        min={0}
        max={1}
        step={0.001}
        value={gain.value}
        onPointerDown={gain.onDragStart}
        onChange={(event) => gain.onDrag(event.currentTarget.valueAsNumber)}
        onPointerUp={gain.onDragEnd}
        onPointerCancel={gain.onDragEnd}
      />
      <output>{gain.text}</output>
    </label>
  );
}

function LabelField() {
  const label = useValue("label");
  // The text the user is typing, from their first key until the field loses
  // the focus; null otherwise, when the field shows the label as the plugin
  // holds it, so a load or an undo shows at once.
  const [draft, setDraft] = useState<string | null>(null);
  // Stores only text that differs from the label, so a click into the field
  // and out stores nothing.
  const commit = () => {
    if (draft !== null && draft !== label.value) label.set(draft);
    setDraft(null);
  };
  return (
    <label className="row">
      <span className="name">Label</span>
      <input
        type="text"
        value={draft ?? label.value ?? ""}
        onChange={(event) => setDraft(event.currentTarget.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          // Enter ends the edit: the blur stores it, once. Not while an input
          // method composes, where Enter picks the text being composed.
          if (event.key === "Enter" && !event.nativeEvent.isComposing) event.currentTarget.blur();
        }}
      />
    </label>
  );
}

function Version() {
  const { run, data } = useCommand("version");
  useEffect(() => {
    run().catch(() => {});
  }, [run]);
  return <span className="version">{data === undefined ? "" : `v${data}`}</span>;
}

function Gain() {
  const failure = useParam("gain").failure;
  if (failure) return <p className="failure">{failure.message}</p>;
  return (
    <main>
      <header>
        <h1>Hapto Example Gain</h1>
        <Version />
      </header>
      <GainSlider />
      <LabelField />
      <KeyboardLine />
    </main>
  );
}

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <Gain />
    </StrictMode>,
  );
}
