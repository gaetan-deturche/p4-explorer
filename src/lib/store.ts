//! Generic cache layer. SQLite (backend) is the durable source of truth; an
//! in-memory Map and localStorage sit in front of it as fast, evictable mirrors
//! so reads paint instantly. Because localStorage is only a cache now, hitting
//! its size cap is no longer data loss — it's a miss that falls back to a ~ms
//! SQLite read. Keys are `scope` + `key`; a scope groups entries you clear
//! together (e.g. one client's tree).

import { invoke } from "@tauri-apps/api/core";

const mem = new Map<string, string>(); // "scope:key" -> json
const lk = (scope: string, key: string) => `${scope}:${key}`;

/** Synchronous read from the fast layer only (memory → localStorage). Returns
 *  null on a miss — call `cacheGet` for the SQLite fallback. */
export function cacheGetSync(scope: string, key: string): string | null {
  const k = lk(scope, key);
  const m = mem.get(k);
  if (m !== undefined) return m;
  try {
    const s = localStorage.getItem(k);
    if (s !== null) {
      mem.set(k, s);
      return s;
    }
  } catch {
    /* storage unavailable */
  }
  return null;
}

/** Full read: memory → localStorage → SQLite. A SQLite hit rehydrates the fast
 *  layers so subsequent reads are synchronous. */
export async function cacheGet(scope: string, key: string): Promise<string | null> {
  const sync = cacheGetSync(scope, key);
  if (sync !== null) return sync;
  try {
    const db = await invoke<string | null>("cache_get", { scope, key });
    if (db !== null) {
      const k = lk(scope, key);
      mem.set(k, db);
      try {
        localStorage.setItem(k, db);
      } catch {
        /* over cap — fine, SQLite still has it */
      }
    }
    return db;
  } catch {
    return null;
  }
}

/** Write to every layer: memory + localStorage (fast) and SQLite (durable). */
export function cacheSet(scope: string, key: string, json: string): void {
  const k = lk(scope, key);
  mem.set(k, json);
  try {
    localStorage.setItem(k, json);
  } catch {
    /* over cap — fine, SQLite is the source of truth */
  }
  invoke("cache_set", { scope, key, json }).catch(() => {});
}

/** Drop every entry in a scope from all layers. */
export function cacheClearScope(scope: string): void {
  const prefix = scope + ":";
  for (const k of [...mem.keys()]) if (k.startsWith(prefix)) mem.delete(k);
  try {
    const del: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k && k.startsWith(prefix)) del.push(k);
    }
    del.forEach((k) => localStorage.removeItem(k));
  } catch {
    /* storage unavailable */
  }
  invoke("cache_clear", { scope }).catch(() => {});
}
