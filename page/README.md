# @hapto/page

The page side of a Hapto plugin UI: a client for the native protocol and React
hooks (`useParam`, `useValue`, `useEvent`, `useCommand`) typed against the
page's own parameter, value, event and command maps. Rust owns the parameters
and the values; the page only mirrors and edits them.

## The Transport seam

The package talks to the native side through `Transport` (`send` a
`PageMessage`, `listen` for `NativeMessage`s), which keeps the order of messages
both ways, and imports nothing else. `memoryTransport()` is for tests; no
browser mock of the native side is built. `webviewTransport()` is for a page in
a plugin's window, whose native side is the `hapto-webview` crate: it posts
through `window.ipc` and installs `window.__hapto` for the native side to
deliver through. A transport for a standalone app is planned. The native side
is `Dispatcher`, in the `hapto-bridge` crate. The message types in
`src/generated/protocol.ts` are generated from Rust; do not edit them.

## Recipe

Implement a `Transport` for your host, create the client from it, then call
`createHooks<Params, Events, Commands, Values>(client)` once at module scope
and export the four hooks it returns. `Params` is the `Params` type of the
plugin's generated parameter map (any interface keyed by id works). `Events` is
an interface keyed by id, whose values are payload types. `Commands` maps each
name to `{ args, returns }`: `args: void` for none, and `returns: null` for a
command with no result, which resolves `null` because the native side always
sends a value. `Values` maps the key of each value the plugin binds for its
page (`Values::value` in `hapto-bridge`, `Page::value` in `hapto-webview`) to
the type serde writes for it. Components call `useParam("gain")`,
`useValue("label")`, `useEvent("clipWarning", handler)` and
`useCommand("getOrder")`; an id that is not in its map is a compile error.

The client sends `ready` after it starts listening, and again every second until
a snapshot or a `reject` arrives. It holds its writes and calls until the
snapshot arrives and then sends them in order, bounded as `src/held.ts` states
(`HELD_PER_PARAM`, `HELD_INVOKES`); a reject drops them and rejects the calls.
It limits every position to 0 to 1, which is all the native side accepts. It
fails visibly (`client.failure()`, `useParam().failure`) when the native side
rejects its protocol version. The native side answers a `ready` of another
version with a `reject` carrying its own version, and every other `ready` with a
snapshot. It answers every `set` and `gestureEnd` with the parameter's state, so
a control ends on the store, and every `setValue` with the value the plugin
holds, stored or not. `useValue` gives `undefined` until the snapshot arrives,
and after a `set` shows the value set until that answer.

## The keyboard

In a plugin's window the page takes the keyboard when it is clicked, as
WebView2 does, and the host sees no key until it has it back.
`webviewTransport()` gives it back whenever nothing in the page needs keys:
after a pointer's release, a focus that left, or a key pressed while nothing
holds the keyboard, it decides in the next task and posts one fixed text
through `ipc`, outside the protocol, and the native side focuses the host's
window. The next click takes the keyboard again.

What keeps it is the focused element (`needsKeys` in `src/keys.ts`): an
`input` that takes typing and is not read-only, a `textarea` that is not
read-only, editable content, a `select` until a pick, and anything inside an
element marked `data-hapto-keys`. A slider, a button, a link and the page's
background do not, so a focused slider's arrow keys are the host's. Mark a
control of your own that takes keys, or `body` for a page that wants every
key while it has the focus:

```tsx
<canvas tabIndex={0} data-hapto-keys />
```

In a field every key is the page's, Ctrl+Z and Ctrl+S included. Escape is the
control's to handle. A control that reads a modifier while dragged takes it
from the pointer's event (`event.shiftKey`): a key let go after the keyboard
went back never reaches the page. A key pressed while nothing holds the
keyboard still reaches the page's own listeners, its default prevented, and
the keys after it are the host's: a page with a shortcut of its own on the
window marks `body`. Only the WebView transport does any of this; the memory
transport, and a scope with no `ipc` or no document, watch nothing. Make the
transport once per page, at module scope: each one adds a watcher, and none
is removed.

## Not built yet

Each needs a protocol message the Rust side does not have today.

- `useObservable` (revisioned containers): an observable snapshot, an update
  message carrying `{ rev, op, value }`, and a page `setObservable` message for
  writable ones.
- `useMeter` (streamed meter batches): a `meter` native message with a sequence
  number and a batch of values, and a subscribe/unsubscribe page message.
- `useMenu` and `HaptoContextMenu` (native context menus): a page `menuGather`
  or `menuShow` message naming the target, and a native reply listing the
  entries and a `menuInvoke` page message for the pick.
