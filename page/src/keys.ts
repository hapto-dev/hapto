// Who has the keyboard while a page is in a plugin's window.
//
// WebView2 takes the keyboard when the page is clicked, and keeps it from the
// host until the user clicks the host. So a page gives it back whenever
// nothing in it needs keys: after a click, a drag or a blur that leaves no
// field focused, the watcher sends the hand-back, and the native side gives
// the keyboard to the host's window. The next click on the page takes it
// again. Each event is answered in the next task, never in its own handler:
// a click on a field's label, or a control that makes and focuses a field on
// a click, moves the focus after the pointer's release, in the same task.

/** The text the page posts to give the keyboard back to the host. It is no
    protocol message, which is tagged by `type`; the native side's shell acts
    on it, and nothing else sees it. */
export const HAND_BACK = '{"shell":"releaseKeys"}';

/** The input types that take a pointer, not typing: focused, they hold no
    keys. A slider is dragged, and the host's arrow keys are worth more. */
const POINTER_INPUTS = new Set(["range", "checkbox", "radio", "button", "submit", "reset", "color", "file", "image"]);

/** Whether `element`, focused, needs the keyboard: a field that takes typing
    and is not read-only, editable content, a `select`, or anything inside an
    element marked `data-hapto-keys`, the claim of a control of the author's
    own. Anything else, the page's background and a slider included, does
    not. */
export function needsKeys(element: Element | null): boolean {
  if (!element) return false;
  if (element.closest("[data-hapto-keys]")) return true;
  const editable = element.closest("[contenteditable]");
  if (editable && editable.getAttribute("contenteditable") !== "false") return true;
  switch (element.tagName) {
    case "INPUT": {
      const input = element as HTMLInputElement;
      return !POINTER_INPUTS.has(input.type) && !input.readOnly;
    }
    case "TEXTAREA":
      return !(element as HTMLTextAreaElement).readOnly;
    case "SELECT":
      return true;
    default:
      return false;
  }
}

/** What the watcher reads: a window, or a test's jsdom window. */
export interface KeyScope {
  document: { activeElement: Element | null; hasFocus(): boolean };
  addEventListener(type: string, listener: (event: Event) => void, capture: boolean): void;
  removeEventListener(type: string, listener: (event: Event) => void, capture: boolean): void;
  setTimeout(handler: () => void, timeout: number): number;
  clearTimeout(id: number): void;
}

/** Watches the page's events on `scope` and calls `send` whenever the page
    has the keyboard and nothing in it needs keys: no pointer is down and
    the focused element does not need them (`needsKeys`), or it is a
    `select` whose pick was just made. Returns the function that stops it.

    Three events ask, in the capture phase so a control that stops one hides
    nothing: a pointer's release, a focus that left, and a key pressed while
    nothing holds the keyboard. That key is the backstop, for a keyboard that
    arrived with no click: its default is prevented, so it presses no button
    and scrolls nothing, and the keys after it are the host's. */
export function watchKeys(scope: KeyScope, send: () => void): () => void {
  const { document } = scope;
  let pointerDown = false;
  // A select whose pick was made: it keeps the focus and needs no keys until
  // it is clicked or focused again.
  let picked: Element | null = null;
  let waiting: number | undefined;

  const holds = () => {
    const focused = document.activeElement;
    return pointerDown || (focused !== picked && needsKeys(focused));
  };
  const decide = () => {
    waiting = undefined;
    if (document.hasFocus() && !holds()) send();
  };
  const ask = () => {
    waiting ??= scope.setTimeout(decide, 0);
  };
  const released = () => {
    pointerDown = false;
    ask();
  };

  const listeners: [string, (event: Event) => void][] = [
    [
      "pointerdown",
      () => {
        pointerDown = true;
        picked = null;
      },
    ],
    ["pointerup", released],
    ["pointercancel", released],
    [
      "focusin",
      () => {
        picked = null;
      },
    ],
    ["focusout", ask],
    [
      "change",
      (event) => {
        const target = event.target as Element | null;
        if (target?.tagName !== "SELECT") return;
        picked = target;
        // The press that opened the list may have its release taken by the
        // list's own window, so the page never hears it.
        pointerDown = false;
        ask();
      },
    ],
    [
      "keydown",
      (event) => {
        if (!document.hasFocus() || holds()) return;
        event.preventDefault();
        ask();
      },
    ],
  ];
  for (const [type, listener] of listeners) scope.addEventListener(type, listener, true);
  return () => {
    for (const [type, listener] of listeners) scope.removeEventListener(type, listener, true);
    if (waiting !== undefined) scope.clearTimeout(waiting);
  };
}
