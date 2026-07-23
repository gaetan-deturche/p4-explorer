//! Server-list persistence for the Server dropdown (remembered P4PORTs).

const SERVERS_KEY = "p4:servers";

export function loadServers(): string[] {
  try {
    const s = localStorage.getItem(SERVERS_KEY);
    return s ? (JSON.parse(s) as string[]) : [];
  } catch {
    return [];
  }
}

export function saveServers(list: string[]) {
  try {
    localStorage.setItem(SERVERS_KEY, JSON.stringify(list));
  } catch {
    /* quota / disabled: ignore */
  }
}

/** Return `list` with `port` appended (deduped, trimmed) — same ref if unchanged. */
export function withServer(list: string[], port: string): string[] {
  const v = port.trim();
  return v && !list.includes(v) ? [...list, v] : list;
}

export function withoutServer(list: string[], port: string): string[] {
  return list.filter((s) => s !== port);
}
