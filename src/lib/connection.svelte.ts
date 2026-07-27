//! Connection feature store: connect/reconnect, the workspace (client) list and
//! selection, the remembered-servers list + switching, workspace stream switch,
//! and the keep-alive/health-check poll. The `conn` object itself stays in the
//! page (it is two-way bound by dialogs); this store drives everything around it.

import { p4, pickFolder as pickFolderCmd, type P4Conn, type P4Record } from "$lib/p4";
import { loadServers, loadServersAsync, saveServers, withServer, withoutServer } from "$lib/servers";
import {
  loadClientFor,
  saveClientFor,
  loadClientsFor,
  saveClientsFor,
  saveLastServer,
  loadView,
  saveView,
  loadUserFor,
  saveUserFor,
  loadCharsetFor,
  saveCharsetFor,
} from "$lib/nav";
import { browse } from "$lib/browse.svelte";
import { history } from "$lib/history.svelte";
import { pending } from "$lib/pending.svelte";

type Tab = "history" | "pending" | "streams" | "log" | "notes";
type Hooks = {
  conn: () => P4Conn;
  getTab: () => Tab;
  setConnError: (m: string) => void; // persistent connection banner ("" clears)
  setNotice: (m: string, ms?: number) => void;
  setOptionsOpen: (v: boolean) => void;
  getSyncing: () => boolean;
  setSyncing: (v: boolean) => void;
  askConfirm: (msg: string, title?: string, ok?: string) => Promise<boolean>;
  // Ask for login credentials (user + password). Resolves null if cancelled.
  promptLogin: (port: string, user: string) => Promise<{ user: string; password: string } | null>;
};

let h: Hooks | null = null;
let keepAliveId: number | null = null;
let focused = true; // window focus; slows background polling
const KEEPALIVE_MS_FOCUS = 20_000;
const KEEPALIVE_MS_BG = 120_000;
function keepAliveMs(): number {
  return focused ? KEEPALIVE_MS_FOCUS : KEEPALIVE_MS_BG;
}

/** Prepend the `ssl:` prefix Perforce needs when the user omits a protocol —
 *  most servers are SSL and typing a bare host:port otherwise fails to connect. */
function normalizePort(port: string): string {
  const v = port.trim();
  if (!v) return v;
  return /^(ssl|tcp)[46]?:/i.test(v) ? v : "ssl:" + v;
}

/** If `msg` is a unicode-mismatch error, flip conn.charset to the fix and return
 *  true (so the caller retries). A unicode client hitting a non-unicode server
 *  drops its charset ("none"); a plain client hitting a unicode server enables
 *  one ("utf8"). Returns false for unrelated errors. */
function adjustCharset(conn: P4Conn, msg: string): boolean {
  if (/unicode clients? require|requires? a unicode enabled server/i.test(msg) && conn.charset !== "none") {
    conn.charset = "none";
    return true;
  }
  if (/only unicode enabled clients|unicode enabled client/i.test(msg) && conn.charset !== "utf8") {
    conn.charset = "utf8";
    return true;
  }
  return false;
}

let connected = $state(false);
let busy = $state(false);
let serverVersion = $state("");
let clients = $state<P4Record[]>([]);
let servers = $state<string[]>([]);
let localClients = $state<Set<string>>(new Set()); // clients bound to this machine (Host)
let clientHost = ""; // this machine's hostname (p4 info clientHost)

// Apply a client list: flag the ones bound to this machine (Host match) and
// sort those first.
function setClientList(list: P4Record[]) {
  localClients = new Set(
    clientHost ? list.filter((c) => (c.Host ?? "").trim() === clientHost).map((c) => c.client) : [],
  );
  clients = [...list].sort((a, b) => {
    const la = localClients.has(a.client) ? 0 : 1;
    const lb = localClients.has(b.client) ? 0 : 1;
    return la - lb || a.client.localeCompare(b.client);
  });
}

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
  }, keepAliveMs());
}

export const connection = {
  init(hooks: Hooks) {
    h = hooks;
    servers = loadServers();
    // If localStorage was reset, recover the list from the SQLite source of truth.
    if (!servers.length) loadServersAsync().then((l) => (servers = l.length ? l : servers));
    // Ease off background polling when the app isn't focused.
    if (typeof window !== "undefined") {
      window.addEventListener("focus", () => connection.setFocused(true));
      window.addEventListener("blur", () => connection.setFocused(false));
    }
  },
  /** Window focus changed → re-pace the keep-alive and the offline scan. */
  setFocused(v: boolean) {
    if (v === focused) return;
    focused = v;
    if (keepAliveId !== null) startKeepAlive(); // re-arm keep-alive at the new rate
    pending.setFocused(v);
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
  get localClients() {
    return localClients;
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
    // If we forgot the server we're on, drop the connection so it doesn't linger
    // as the selected entry in the dropdown.
    if (h && port === h.conn().port) {
      connection.stopKeepAlive();
      connected = false;
      const conn = h.conn();
      conn.client = "";
      conn.port = "";
      browse.reset();
    }
  },

  async connect(opts?: { skipAuth?: boolean; skipReselect?: boolean }) {
    if (!h) return;
    const conn = h.conn();
    // `skipReselect`: a background validation of an already-open (optimistically
    // shown) workspace — don't flip the UI into a connecting/empty state.
    if (!opts?.skipReselect) {
      busy = true;
      clients = []; // don't show the previous server's workspaces while switching
      localClients = new Set();
    }
    connection.stopKeepAlive(); // no health-check poll while (re)connecting
    h.setConnError("");
    try {
      // Seed the server dropdown: adopt the ambient P4PORT if none was set.
      if (!conn.port) {
        const env = await p4.envPort(conn).catch(() => "");
        if (env) conn.port = env;
      }
      if (conn.port) conn.port = normalizePort(conn.port); // ssl: prefix if omitted
      conn.charset = loadCharsetFor(conn.port); // this server's remembered charset
      const origPort = conn.port;
      // Connect, auto-fixing first-connect papercuts: a server that needs the
      // `ssl:` prefix, an untrusted fingerprint (`p4 trust`), and a unicode
      // client/server charset mismatch.
      let info: P4Record[] | null = null;
      for (let attempt = 0; attempt < 5 && !info; attempt++) {
        try {
          info = await p4.info(conn);
        } catch (e) {
          const msg = String(e);
          if (/SSL/i.test(msg) && conn.port && !conn.port.startsWith("ssl:")) {
            conn.port = "ssl:" + conn.port;
          } else if (/trust|fingerprint|authenticity|P4TRUST/i.test(msg)) {
            await p4.trust(conn).catch(() => {});
          } else if (adjustCharset(conn, msg)) {
            // charset flipped; retry
          } else {
            throw e;
          }
        }
      }
      if (!info) info = await p4.info(conn); // last try; throws → outer catch
      const i = info[0] ?? {};
      serverVersion = i.serverVersion ?? "";
      // Pick the user for this server: a remembered one wins; else the account
      // an existing ticket was issued to (e.g. a prior P4V login) — that server
      // may not accept the ambient P4USER — and only then p4 info's default.
      if (!conn.user) {
        conn.user = (await p4.ticketUser(conn).catch(() => "")) || i.userName || "";
      }
      // If we corrected the port (e.g. added ssl:), replace the remembered entry.
      if (conn.port !== origPort && origPort) connection.forgetServer(origPort);
      connection.rememberServer(conn.port);
      // Ensure this server's session is authenticated; prompt for a password and
      // log in on demand (per-server tickets can be missing or expired). Skip the
      // check when the caller (relogin) has just logged in — some servers don't
      // report the fresh ticket via `login -s` immediately, which re-prompted.
      const authed = opts?.skipAuth || (await p4.loginStatus(conn).catch(() => true));
      if (!authed) {
        const cred = await h.promptLogin(conn.port, conn.user);
        if (!cred) {
          connected = false;
          return; // user cancelled — stay disconnected, no success toast
        }
        conn.user = cred.user;
        let loggedIn = false;
        for (let attempt = 0; attempt < 3 && !loggedIn; attempt++) {
          try {
            await p4.login(conn, cred.password);
            loggedIn = true;
          } catch (e) {
            // A unicode charset mismatch surfaces here (info doesn't negotiate
            // charset, login does); flip charset and retry, else fail.
            if (!adjustCharset(conn, String(e))) {
              connected = false;
              h.setConnError(`Login failed: ${String(e)}`);
              return;
            }
          }
        }
        if (!loggedIn) {
          connected = false;
          h.setConnError("Login failed.");
          return;
        }
      }
      connected = true;
      h.setOptionsOpen(false);
      startKeepAlive();
      saveLastServer(conn.port); // restore THIS server on next launch, regardless of workspace
      saveUserFor(conn.port, conn.user); // remember this server's user
      saveCharsetFor(conn.port, conn.charset); // and its charset choice
      browse.ensureDepotIndex(); // background: build the whole-depot search index
      // Flag workspaces bound to THIS machine (client Host == this host) and
      // sort those first — Host-locked is the accurate signal; a shared client
      // (empty Host) is not "this machine" even if its Root folder exists here.
      clientHost = (i.clientHost ?? "").trim();
      // Prefer the workspace the user last used on this server; fall back to the
      // client reported by `p4 info`.
      const saved = loadClientFor(conn.port);
      const cn = i.clientName;
      const pick = (list: P4Record[]) =>
        saved && list.some((c) => c.client === saved)
          ? saved
          : cn && cn !== "*unknown*" && list.some((c) => c.client === cn)
            ? cn
            : "";

      // Show the cached workspace list instantly, then refresh from the server in
      // the BACKGROUND — so selecting the workspace doesn't wait on a `p4 clients`
      // round-trip (noticeable on a remote server like Epic).
      const cachedClients = loadClientsFor(conn.port);
      if (cachedClients.length && !opts?.skipReselect) setClientList(cachedClients);
      const refreshClients = p4
        .clients(conn)
        .then((fresh) => {
          setClientList(fresh);
          saveClientsFor(conn.port, fresh);
        })
        .catch(() => {});

      if (opts?.skipReselect) {
        await refreshClients; // workspace already open — just refresh list + Host marks
      } else {
        const cachedTarget = pick(clients);
        if (cachedTarget) {
          await connection.selectClient(cachedTarget); // open now, from the cached list
        } else {
          await refreshClients; // no usable cache — need the fresh list to pick a target
          const t = pick(clients);
          if (t) await connection.selectClient(t);
        }
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
    conn.user = loadUserFor(port); // this server's remembered user ("" → ambient)
    conn.charset = loadCharsetFor(port);
    conn.client = "";
    browse.reset();

    // Optimistic fast path for a KNOWN server: show its cached workspace list and
    // open the saved workspace immediately (its views paint from the store), then
    // validate auth + refresh in the background. Unknown/first-time servers take
    // the full blocking connect (SSL/trust/charset first-connect fixup).
    const cachedClients = loadClientsFor(port);
    const savedClient = loadClientFor(port);
    const hasTarget =
      !!savedClient && cachedClients.some((c) => c.client === savedClient && c.Stream);
    if (hasTarget) {
      connected = true; // optimistic — reverted by connect() below if auth fails
      setClientList(cachedClients);
      await connection.selectClient(savedClient); // opens the cached workspace instantly
      void connection.connect({ skipReselect: true }); // validate + refresh in background
    } else {
      await connection.connect();
    }
  },
  /** Create a new stream workspace bound to this machine, then switch to it. */
  async createWorkspace(name: string, root: string, stream: string) {
    if (!h) return;
    const conn = h.conn();
    try {
      await p4.newClient(conn, name.trim(), root.trim(), stream.trim());
    } catch (e) {
      h.setConnError(`Create workspace failed: ${String(e)}`);
      return;
    }
    h.setNotice(`Workspace ${name.trim()} created.`);
    const list = await p4.clients(conn).catch(() => [] as P4Record[]); // pick up the new client
    setClientList(list);
    saveClientsFor(conn.port, list);
    await connection.selectClient(name.trim());
  },
  /** A P4V-style default workspace name: user_host_NNNN (fresh number each call). */
  suggestWorkspaceName(): string {
    const clean = (s: string) => s.replace(/[^A-Za-z0-9_.-]+/g, "").trim();
    const user = clean(h?.conn().user ?? "") || "user";
    const host = clean(clientHost) || "host";
    const n = 1000 + Math.floor(Math.random() * 9000);
    return `${user}_${host}_${n}`;
  },
  /** Open the native folder-picker for the New-workspace Root field. */
  pickFolder(start: string) {
    return pickFolderCmd(start);
  },
  /** Existing streams on the current server, for the New-workspace stream picker. */
  async loadStreams(): Promise<{ stream: string; name: string }[]> {
    if (!h) return [];
    const rows = await p4.streams(h.conn()).catch(() => [] as P4Record[]);
    return rows
      .filter((r) => r.Stream)
      .map((r) => ({ stream: r.Stream, name: r.Name ?? r.Stream }));
  },
  /** Remember `port` and connect. Adding a server goes through the login flow so
   *  the user picks the account for it (the ambient P4USER may have no access to
   *  a newly-added server). */
  async addAndSwitch(port: string) {
    const v = normalizePort(port);
    if (!v) return;
    connection.rememberServer(v);
    await connection.relogin(v);
  },
  /** Log in to `port` (from Add server or Options → Re-login): point at that
   *  server, prompt for the user + password, establish SSL trust, log in (fixing
   *  a charset mismatch if needed), then connect with the fresh ticket. */
  async relogin(port: string) {
    if (!h) return;
    const conn = h.conn();
    connection.stopKeepAlive(); // avoid a background "lost connection" while the login prompt is open
    if (port && port !== conn.port) {
      conn.port = normalizePort(port);
      conn.user = loadUserFor(conn.port);
      conn.charset = loadCharsetFor(conn.port);
      conn.client = "";
      browse.reset();
    }
    // Prefill the user from an existing ticket for this server if we have none.
    if (!conn.user) conn.user = await p4.ticketUser(conn).catch(() => "");
    const cred = await h.promptLogin(conn.port, conn.user);
    if (!cred) return;
    conn.user = cred.user;
    await p4.trust(conn).catch(() => {}); // accept the SSL fingerprint on first use
    let ok = false;
    for (let i = 0; i < 3 && !ok; i++) {
      try {
        await p4.login(conn, cred.password);
        ok = true;
      } catch (e) {
        if (!adjustCharset(conn, String(e))) {
          h.setConnError(`Login failed: ${String(e)}`);
          return;
        }
      }
    }
    if (!ok) {
      h.setConnError("Login failed.");
      return;
    }
    h.setNotice("Logged in.");
    saveCharsetFor(conn.port, conn.charset); // keep the charset login settled on
    await connection.connect({ skipAuth: true }); // fresh ticket → don't re-prompt
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
