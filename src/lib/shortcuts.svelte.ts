//! Keyboard shortcuts: one registry of actions, their default keys, and the
//! user's rebindings.
//!
//! Everything that needs a key reads it from here — the window-level dispatcher,
//! the menu-bar accelerators, the context-menu labels — so a rebinding shows up
//! everywhere at once and nothing can drift out of sync with what actually fires.
//!
//! Bindings are stored as a normalized string ("Ctrl+F", "F5", "Ctrl+Shift+P",
//! "Alt+ArrowUp"): modifiers in a fixed order, then the key. Comparing an event
//! is then a string equality, and the same string is what the UI displays.

import { cacheGet, cacheSet } from "$lib/store.svelte";

/** Where a shortcut applies. The dispatcher only fires an action whose scope is
 *  currently meaningful, so `Ctrl+N` can mean "new changelist" on the Pending
 *  tab without being dead weight elsewhere. */
export type Scope = "app" | "pending" | "files";

export interface ActionDef {
  id: string;
  label: string;
  /** Grouping in the Options list. */
  group: string;
  scope: Scope;
  /** The out-of-the-box binding ("" = unbound by default). */
  def: string;
  /** Confirmed but irreversible-ish: shown apart in Options so a rebinding is a
   *  deliberate act. */
  destructive?: boolean;
}

/** Every bindable action. Adding one here is enough for it to appear in Options;
 *  the dispatcher maps the id to the actual work. */
export const ACTIONS: ActionDef[] = [
  { id: "closeWindow", label: "Close the window", group: "Windows", scope: "app", def: "Ctrl+W" },
  { id: "refresh", label: "Refresh everything", group: "Workspace", scope: "app", def: "F5" },
  { id: "sync", label: "Sync workspace…", group: "Workspace", scope: "app", def: "Ctrl+Shift+S" },
  { id: "applyPatch", label: "Apply patch…", group: "Workspace", scope: "app", def: "Ctrl+Shift+P" },
  { id: "options", label: "Options…", group: "Workspace", scope: "app", def: "Ctrl+," },
  { id: "search", label: "Focus the search box", group: "Navigation", scope: "app", def: "Ctrl+F" },
  { id: "tabHistory", label: "History tab", group: "Navigation", scope: "app", def: "Ctrl+1" },
  { id: "tabPending", label: "Pending tab", group: "Navigation", scope: "app", def: "Ctrl+2" },
  { id: "tabReviews", label: "Reviews tab", group: "Navigation", scope: "app", def: "Ctrl+3" },
  { id: "tabStreams", label: "Streams tab", group: "Navigation", scope: "app", def: "Ctrl+4" },
  { id: "tabLog", label: "Commands tab", group: "Navigation", scope: "app", def: "Ctrl+5" },
  { id: "newChange", label: "New changelist…", group: "Pending", scope: "pending", def: "Ctrl+N" },
  { id: "rename", label: "Rename changelist…", group: "Pending", scope: "pending", def: "F2" },
  { id: "diff", label: "Diff the selected file", group: "Files", scope: "files", def: "Ctrl+D" },
  { id: "copyPath", label: "Copy the depot path", group: "Files", scope: "files", def: "Ctrl+C" },
  { id: "blame", label: "Blame the selected file…", group: "Files", scope: "files", def: "Ctrl+B" },
  { id: "fileHistory", label: "File history…", group: "Files", scope: "files", def: "Ctrl+H" },
  {
    id: "submit",
    label: "Submit the changelist…",
    group: "Pending",
    scope: "pending",
    def: "Ctrl+Enter",
    destructive: true,
  },
  {
    id: "revert",
    label: "Revert the selection…",
    group: "Files",
    scope: "files",
    def: "Delete",
    destructive: true,
  },
];

const KEY = "shortcuts"; // store scope `nav`
let overrides = $state<Record<string, string>>({});

/** Modifier order is fixed so "Shift+Ctrl+F" and "Ctrl+Shift+F" cannot both
 *  exist and mean the same thing. */
export function describe(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  let k = e.key;
  if (k === " ") k = "Space";
  // A letter is reported lower-case unless Shift is down; normalize so the
  // binding text does not depend on which it was.
  if (k.length === 1) k = k.toUpperCase();
  if (["Control", "Alt", "Shift", "Meta"].includes(k)) return ""; // modifier alone
  parts.push(k);
  return parts.join("+");
}

export const shortcuts = {
  /** Load the user's rebindings. Authoritative (SQLite): the localStorage mirror
   *  is bounded and silently drops writes once full, which is exactly how the
   *  editor preference used to lose itself between restarts. */
  async init() {
    const saved = await cacheGet("nav", KEY);
    if (!saved) return;
    try {
      const parsed = JSON.parse(saved) as Record<string, string>;
      if (parsed && typeof parsed === "object") overrides = parsed;
    } catch {
      /* corrupt — defaults stand */
    }
  },
  /** The binding in force for an action ("" when unbound). */
  key(id: string): string {
    if (id in overrides) return overrides[id];
    return ACTIONS.find((a) => a.id === id)?.def ?? "";
  },
  /** For a menu label: the binding, or "" so nothing is shown when unbound. */
  accel(id: string): string {
    return shortcuts.key(id);
  },
  /** Which action a keystroke means, honouring `scopes` (the ones currently
   *  meaningful). Returns "" for no match. */
  match(e: KeyboardEvent, scopes: Scope[]): string {
    const pressed = describe(e);
    if (!pressed) return "";
    for (const a of ACTIONS) {
      if (!scopes.includes(a.scope)) continue;
      if (shortcuts.key(a.id) === pressed) return a.id;
    }
    return "";
  },
  /** Actions currently bound to `key`, other than `except` — a rebinding that
   *  would collide has to be visible before it is accepted. */
  clashes(key: string, except: string): ActionDef[] {
    if (!key) return [];
    return ACTIONS.filter((a) => a.id !== except && shortcuts.key(a.id) === key);
  },
  /** Rebind. An empty key unbinds; the same key as the default drops the
   *  override so a later change of default is picked up. */
  set(id: string, key: string) {
    const def = ACTIONS.find((a) => a.id === id)?.def ?? "";
    const next = { ...overrides };
    if (key === def) delete next[id];
    else next[id] = key;
    overrides = next;
    cacheSet("nav", KEY, JSON.stringify(next));
  },
  /** Back to the shipped binding. */
  reset(id: string) {
    shortcuts.set(id, ACTIONS.find((a) => a.id === id)?.def ?? "");
  },
  resetAll() {
    overrides = {};
    cacheSet("nav", KEY, "{}");
  },
  /** True when the action's binding differs from the shipped one. */
  isCustom(id: string): boolean {
    return id in overrides;
  },
};

/** Should a keystroke be ignored because the user is typing? A shortcut must
 *  never steal a character from an input, and Ctrl+C in a text box has to keep
 *  meaning copy. */
export function typingInto(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el.isContentEditable;
}
