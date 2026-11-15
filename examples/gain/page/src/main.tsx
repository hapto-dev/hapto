// The gain's page: a slider on the gain, which drags it as a user would, and
// a field for the plugin's label, through two commands.

import { StrictMode, useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { createClient, createHooks, webviewTransport } from "seam-page";

import type { Params } from "./generated/params";
import "./styles.css";

/** The commands `examples/gain/src/page.rs` adds. */
interface Commands {
  getLabel: { args: void; returns: string };
  setLabel: { args: string; returns: null };
}

const client = createClient(webviewTransport());
const { useParam, useCommand } = createHooks<Params, Record<string, never>, Commands>(client);

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
  const { run: fetchLabel } = useCommand("getLabel");
  const { run: storeLabel, error } = useCommand("setLabel");
  const [label, setLabel] = useState("");
  // The label as the plugin last had it, read or stored; null until read.
  // The page hears of no change the host makes, such as a load, so it reads
  // the label again on focus, and stores only text that differs from it: a
  // click into the field and out never writes an old label over a new one.
  const known = useRef<string | null>(null);
  // Counts the stores sent, so a read that a store overtook is dropped.
  const stores = useRef(0);
  const read = useCallback(() => {
    const asked = stores.current;
    fetchLabel().then(
      (current) => {
        if (stores.current !== asked) return;
        const before = known.current;
        known.current = current;
        // Text typed since the last read is the user's, and stays.
        setLabel((shown) => (before === null || shown === before ? current : shown));
      },
      () => {},
    );
  }, [fetchLabel]);
  useEffect(read, [read]);
  const commit = () => {
    const before = known.current;
    if (before === null || label === before) return;
    known.current = label;
    stores.current += 1;
    storeLabel(label).catch(() => {
      known.current = before;
    });
  };
  return (
    <label className="row">
      <span className="name">Label</span>
      <input
        type="text"
        value={label}
        onChange={(event) => setLabel(event.currentTarget.value)}
        onFocus={read}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
        }}
      />
      <output>{error ? "not saved" : ""}</output>
    </label>
  );
}

function Gain() {
  const failure = useParam("gain").failure;
  if (failure) return <p className="failure">{failure.message}</p>;
  return (
    <main>
      <h1>Seam Example Gain</h1>
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
