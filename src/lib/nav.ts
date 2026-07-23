//! Navigation persistence (localStorage, best-effort): the last server + the
//! last workspace used on each server, and the last view (tab + selection) for
//! each workspace. Lets the app reopen where the user left off and return to a
//! workspace's last view when switching back to it.

export interface ViewState {
  tab: "history" | "pending" | "streams" | "repo";
  treePath: string; // selected depot path (tree highlight + history subject)
  histMode: "folder" | "file";
}

const LAST_SERVER = "nav:lastServer";
const clientKey = (server: string) => `nav:client:${server}`;
const userKey = (server: string) => `nav:user:${server}`;
const charsetKey = (server: string) => `nav:charset:${server}`;
const viewKey = (client: string) => `nav:view:${client}`;

function get(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function set(key: string, val: string): void {
  try {
    localStorage.setItem(key, val);
  } catch {
    /* storage unavailable / full — navigation memory is best-effort */
  }
}

export function loadLastServer(): string {
  return get(LAST_SERVER) ?? "";
}
export function saveLastServer(server: string): void {
  if (server) set(LAST_SERVER, server);
}

/** The last workspace (client) used on `server`, or "" if none. */
export function loadClientFor(server: string): string {
  return server ? (get(clientKey(server)) ?? "") : "";
}
export function saveClientFor(server: string, client: string): void {
  if (server && client) set(clientKey(server), client);
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
