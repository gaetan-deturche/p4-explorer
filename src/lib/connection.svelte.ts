//! Connection feature store: connect/reconnect, the workspace (client) list and
//! selection, the remembered-servers list + switching, workspace stream switch,
//! and the keep-alive/health-check poll. The `conn` object itself stays in the
//! page (it is two-way bound by dialogs); this store drives everything around it.

import { p4, type P4Conn, type P4Record } from "$lib/p4";
import { loadServers, saveServers, withServer, withoutServer } from "$lib/servers";
import { loadClientFor, saveClientFor, saveLastServer, loadView, saveView } from "$lib/nav";
import { browse } from "$lib/browse.svelte";
import { history } from "$lib/history.svelte";

type Tab = "history" | "pending" | "streams" | "repo";
type Hooks = {
  conn: () => P4Conn;
  getTab: () => Tab;
  setConnError: (m: string) => void; // persistent connection banner ("" clears)
  setNotice: (m: string, ms?: number) => void;
  setOptionsOpen: (v: boolean) => void;
  getSyncing: () => boolean;
  setSyncing: (v: boolean) => void;
  askConfirm: (msg: string, title?: string, ok?: string) => Promise<boolean>;
};

let h: Hooks | null = null;
let keepAliveId: number | null = null;

let connected = $state(false);
let busy = $state(false);
let serverVersion = $state("");
let clients = $state<P4Record[]>([]);
let servers = $state<string[]>([]);

// Each `p4` CLI call reconnects; the first `dirs` on a huge stream root is a
// ~2.7s server-side cold disk read of db.rev. Re-running it periodically keeps
// that cache hot so it doesn't recur mid-session. Doubles as a health check.
function startKeepAlive() {
  if (keepAliveId !== null) clearInterval(keepAliveId);
  keepAliveId = window.setInterval(async () => {
    if (!h) return;
    try {
      await p4.info(h.conn());
      if (!connected) {
        connected = true; // recovered
        h.setConnError("");
      }
      if (browse.rootPath) p4.dirs(h.conn(), browse.rootPath).catch(() => {}); // keep cache warm
    } catch {
      if (connected) {
        connected = false;
        h.setConnError("Lost connection to the Perforce server. Retrying…");
      }
    }
  }, 20000);
}

export const connection = {
  init(hooks: Hooks) {
    h = hooks;
    servers = loadServers();
  },
  get connected() {
    return connected;
  },
  get busy() {
    return busy;
  },
  get serverVersion() {
    return serverVersion;
  },
  get clients() {
    return clients;
  },
  get servers() {
    return servers;
  },

  stopKeepAlive() {
    if (keepAliveId !== null) clearInterval(keepAliveId);
    keepAliveId = null;
  },

  rememberServer(port: string) {
    const next = withServer(servers, port);
    if (next !== servers) {
      servers = next;
      saveServers(servers);
    }
  },
  forgetServer(port: string) {
    servers = withoutServer(servers, port);
    saveServers(servers);
  },

  async connect() {
    if (!h) return;
    const conn = h.conn();
    busy = true;
    h.setConnError("");
    try {
      const info = await p4.info(conn);
      const i = info[0] ?? {};
      serverVersion = i.serverVersion ?? "";
      if (!conn.user && i.userName) conn.user = i.userName;
      // Seed the server dropdown: adopt the ambient P4PORT if none was set.
      if (!conn.port) {
        const env = await p4.envPort(conn).catch(() => "");
        if (env) conn.port = env;
      }
      connection.rememberServer(conn.port);
      connected = true;
      h.setOptionsOpen(false);
      startKeepAlive();
      clients = await p4.clients(conn);
      // Prefer the workspace the user last used on this server; fall back to the
      // client reported by `p4 info`.
      const saved = loadClientFor(conn.port);
      const cn = i.clientName;
      const target =
        saved && clients.some((c) => c.client === saved)
          ? saved
          : cn && cn !== "*unknown*" && clients.some((c) => c.client === cn)
            ? cn
            : "";
      if (target) {
        await connection.selectClient(target);
      }
    } catch (e) {
      connected = false;
      h.setConnError(String(e));
    } finally {
      busy = false;
    }
  },

  /** Select a workspace. `target` is the incoming client (from the picker);
   *  omit it to (re)select the current one (e.g. after a stream switch). */
  async selectClient(target?: string) {
    if (!h) return;
    const conn = h.conn();
    const next = target ?? conn.client;
    const prev = conn.client;
    const fallbackTab = h.getTab(); // used only if the incoming ws has no saved view
    // Persist the OUTGOING workspace's view, and read the INCOMING workspace's
    // saved view BEFORE conn.client changes — so neither the page's save-effect
    // nor the reset below can clobber the view we're about to restore.
    if (prev && prev !== next) {
      saveView(prev, { tab: fallbackTab, treePath: browse.selectedTreePath, histMode: history.mode });
    }
    const saved = loadView(next);
    conn.client = next;
    h.setConnError("");
    browse.reset();
    const rec = clients.find((c) => c.client === next);
    if (!rec) return;
    if (!rec.Stream) {
      h.setConnError("This workspace has no stream. Depot browsing currently requires a stream client.");
      return;
    }
    saveLastServer(conn.port);
    saveClientFor(conn.port, next);
    await browse.openWorkspace(rec.Stream, rec.Root ?? "", saved, fallbackTab);
  },

  async switchServerTo(port: string) {
    if (!h) return;
    const conn = h.conn();
    if (port === conn.port) return;
    conn.port = port;
    conn.client = "";
    browse.reset();
    await connection.connect();
  },
  /** Remember `port` and switch to it (from the "add server" dialog). */
  async addAndSwitch(port: string) {
    const v = port.trim();
    if (!v) return;
    connection.rememberServer(v);
    await connection.switchServerTo(v);
  },

  async switchStream(stream: string) {
    if (!h || !connected || h.getSyncing()) return;
    const conn = h.conn();
    if (
      !(await h.askConfirm(
        `${stream}\n\nThis reconfigures the workspace and syncs to that stream. Open files will block the switch — shelve or submit them first.`,
        "Switch workspace stream",
        "Switch",
      ))
    ) {
      return;
    }
    h.setSyncing(true);
    h.setConnError("");
    try {
      await p4.switch(conn, stream); // throws (surfaced below) if p4 refuses
      clients = await p4.clients(conn); // pick up the client's new Stream
      await connection.selectClient();
      h.setNotice(`Switched workspace to ${stream}.`);
    } catch (e) {
      h.setConnError(String(e));
    } finally {
      h.setSyncing(false);
    }
  },
};
