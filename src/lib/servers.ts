//! Server-list persistence for the Server dropdown (remembered P4PORTs). Backed
//! by the store (SQLite source of truth), so the list survives a localStorage
//! reset — it's critical state, not a disposable cache.

import { cacheGetSync, cacheGet, cacheSet } from "$lib/store";

const SERVERS_KEY = "servers"; // store scope `nav`

export function loadServers(): string[] {
  try {
    return JSON.parse(cacheGetSync("nav", SERVERS_KEY) ?? "[]") as string[];
  } catch {
    return [];
  }
}
/** Server list including the SQLite source of truth (for a lost localStorage). */
export async function loadServersAsync(): Promise<string[]> {
  try {
    return JSON.parse((await cacheGet("nav", SERVERS_KEY)) ?? "[]") as string[];
  } catch {
    return [];
  }
}

export function saveServers(list: string[]) {
  cacheSet("nav", SERVERS_KEY, JSON.stringify(list));
}

/** Return `list` with `port` appended (deduped, trimmed) — same ref if unchanged. */
export function withServer(list: string[], port: string): string[] {
  const v = port.trim();
  return v && !list.includes(v) ? [...list, v] : list;
}

export function withoutServer(list: string[], port: string): string[] {
  return list.filter((s) => s !== port);
}
