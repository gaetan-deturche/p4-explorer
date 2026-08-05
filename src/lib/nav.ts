//! Navigation persistence: the last server + the last workspace used on each
//! server, and the last view (tab + selection) for each workspace. Lets the app
//! reopen where the user left off. Backed by the generic `store` (SQLite source
//! of truth + localStorage/memory fast layers), scope `nav`.

import { cacheGetSync, cacheSet } from "$lib/store.svelte";

export interface ViewState {
  tab: "history" | "pending" | "reviews" | "streams" | "log" | "notes";
  treePath: string; // selected depot path (tree highlight + history subject)
  histMode: "folder" | "file";
}

/** Which views (panes/tabs) are shown. `files` is the left file browser (its
 *  Local/Workspace/Depot source is chosen in the pane, not a separate view). */
export interface Views {
  files: boolean;
  history: boolean;
  pending: boolean;
  reviews: boolean; // Swarm code reviews
  streams: boolean;
  log: boolean; // the p4-command log ("Commands")
  notes: boolean; // the notification history ("Notifications")
}
const DEFAULT_VIEWS: Views = {
  files: true,
  history: true,
  pending: true,
  reviews: true,
  streams: false,
  log: false,
  notes: false,
};
const VIEWS = "views:v3"; // v3: Depot is now a source of the Files pane, not a tab
export function loadViews(): Views {
  const raw = get(VIEWS);
  if (raw) {
    try {
      return { ...DEFAULT_VIEWS, ...JSON.parse(raw) };
    } catch {
      /* corrupt — fall through to defaults */
    }
  }
  return { ...DEFAULT_VIEWS };
}
export function saveViews(v: Views): void {
  set(VIEWS, JSON.stringify(v));
}

const LAST_SERVER = "lastServer";
const clientKey = (server: string) => `client:${server}`;
const userKey = (server: string) => `user:${server}`;
const charsetKey = (server: string) => `charset:${server}`;
const viewKey = (client: string) => `view:${client}`;

// Route through the store (scope `nav`): reads are synchronous from the fast
// layers; writes also persist to SQLite. Keys keep the same `nav:*` localStorage
// layout, so existing data is reused.
function get(key: string): string | null {
  return cacheGetSync("nav", key);
}
function set(key: string, val: string): void {
  cacheSet("nav", key, val);
}

export function loadLastServer(): string {
  return get(LAST_SERVER) ?? "";
}
export function saveLastServer(server: string): void {
  if (server) set(LAST_SERVER, server);
}

const BROWSE_SRC = "browseSource";
/** The Files-pane data source, persisted globally. Defaults to `local`. */
export function loadBrowseSource(): "local" | "workspace" | "depot" {
  const v = get(BROWSE_SRC);
  return v === "workspace" || v === "depot" ? v : "local";
}
export function saveBrowseSource(s: "local" | "workspace" | "depot"): void {
  set(BROWSE_SRC, s);
}

const HIST_COLS = "histCols";
/** Manually-resized History columns (auto-fitted when absent), by column key. */
export function loadHistCols(): Record<string, number> {
  try {
    const v = JSON.parse(get(HIST_COLS) ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}
export function saveHistCols(w: Record<string, number | undefined>): void {
  set(HIST_COLS, JSON.stringify(w));
}

const SHOW_DELETED = "showDeleted";
/** Whether the Files pane lists files deleted at head (off by default). */
export function loadShowDeleted(): boolean {
  return get(SHOW_DELETED) === "1";
}
export function saveShowDeleted(v: boolean): void {
  set(SHOW_DELETED, v ? "1" : "0");
}

/** The last workspace (client) used on `server`, or "" if none. */
export function loadClientFor(server: string): string {
  return server ? (get(clientKey(server)) ?? "") : "";
}
export function saveClientFor(server: string, client: string): void {
  if (server && client) set(clientKey(server), client);
}

const clientsKey = (server: string) => `clients:${server}`;
/** Cached `p4 clients` list for `server` — shown instantly on connect while the
 *  fresh list loads. Records carry client/Host/Root/Stream. */
export function loadClientsFor(server: string): Record<string, string>[] {
  if (!server) return [];
  try {
    const s = get(clientsKey(server));
    return s ? (JSON.parse(s) as Record<string, string>[]) : [];
  } catch {
    return [];
  }
}
export function saveClientsFor(server: string, list: Record<string, string>[]): void {
  if (server) set(clientsKey(server), JSON.stringify(list));
}

/** The user (P4USER) last used on `server`, or "" if none. Servers can differ. */
export function loadUserFor(server: string): string {
  return server ? (get(userKey(server)) ?? "") : "";
}
export function saveUserFor(server: string, user: string): void {
  if (server && user) set(userKey(server), user);
}

/** The charset chosen for `server` ("" ambient, "none", or e.g. "utf8"). */
export function loadCharsetFor(server: string): string {
  return server ? (get(charsetKey(server)) ?? "") : "";
}
export function saveCharsetFor(server: string, charset: string): void {
  if (server) set(charsetKey(server), charset);
}

/** The last view (tab + selection) for `client`, or null. */
export function loadView(client: string): ViewState | null {
  if (!client) return null;
  const raw = get(viewKey(client));
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    if (
      v &&
      typeof v.tab === "string" &&
      typeof v.treePath === "string" &&
      typeof v.histMode === "string"
    ) {
      return v as ViewState;
    }
  } catch {
    /* corrupt entry — ignore */
  }
  return null;
}
export function saveView(client: string, v: ViewState): void {
  if (client) set(viewKey(client), JSON.stringify(v));
}
