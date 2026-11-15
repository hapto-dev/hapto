// The gain's page: a slider on the gain, which drags it as a user would, a
// field for the plugin's label, a value the plugin binds, and the plugin's
// version, through a command.

import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { createClient, createHooks, webviewTransport } from "seam-page";

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
          // Enter ends the edit: the blur stores it, once.
          if (event.key === "Enter") event.currentTarget.blur();
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
        <h1>Seam Example Gain</h1>
        <Version />
      </header>
      <GainSlider />
      <LabelField />
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
