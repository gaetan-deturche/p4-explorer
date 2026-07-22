# P4 Explorer

A dedicated desktop GUI for Perforce Helix Core — the moddable, source-available
client P4V isn't. Built with **Tauri v2 (Rust)** + **SvelteKit (Svelte 5)**.

Its distinguishing feature vs P4V: the **currently-synced revision is marked
inline** everywhere — a green `synced @ <CL>` badge on folder history, a `have
#<rev>` badge with a `▸` marker on the exact row in file history, and per-file
sync dots in the depot browser.

## How it works

The Rust backend shells out to the `p4` CLI with `p4 -ztag -Mj <cmd>`, which
emits one JSON object per record. `src-tauri/src/p4.rs` parses those lines,
splits data from error records (by numeric `severity`), and explodes the
index-suffixed list formats (`filelog` `rev0/change0/...`, `describe`
`depotFile0/...`) into flat per-row records. It reuses the ambient p4
environment (P4PORT / P4USER / tickets), so no credentials are handled.

Commands exposed to the UI (`src-tauri/src/commands.rs`): `info`, `clients`,
`dirs`, `files` (fstat with have/head), `changes`, `pending`, `have_change`
(the CL a path is synced to), `describe`, `filelog`, `fstat`.

## Run (development)

```sh
npm install
npm run tauri dev
```

## Build a standalone installer / exe

```sh
npm run tauri build
# exe:       src-tauri/target/release/p4gui.exe
# installer: src-tauri/target/release/bundle/
```

## Layout

```
src/lib/p4.ts                     typed invoke() wrappers + helpers
src/lib/components/               ConnectionBar, DepotBrowser, HistoryTable, ChangeDetails
src/routes/+page.svelte           three-pane orchestrator
src-tauri/src/p4.rs               p4 CLI runner + JSON/record parsing
src-tauri/src/commands.rs         Tauri command wrappers
```

## Current limitations

- Depot browsing requires a **stream** client. Classic (view-based) clients
  don't yet enumerate a root (the Repo tab still browses any depot path).

## Disclaimer

This is an independent, unofficial tool. It invokes the `p4` command-line client
(which you must install separately) and does not bundle, link, or redistribute
any Perforce software or SDK. It is **not affiliated with or endorsed by Perforce
Software, Inc.** "Perforce", "Helix Core", "P4", and "Swarm" are trademarks of
Perforce Software, Inc., used here only to refer to the software this tool works with.

## License

MIT — see [LICENSE](LICENSE).
