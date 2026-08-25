<script lang="ts">
  import { onMount } from "svelte";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import type { CmdEntry, RunningCmd } from "$lib/cmdlog.svelte";
  import { p4 } from "$lib/p4";

  let {
    entries,
    running,
    onClear,
    onClearRunning,
  }: {
    entries: CmdEntry[];
    /** Commands started and not yet finished, oldest first. */
    running: RunningCmd[];
    onClear: () => void;
    onClearRunning: () => void;
  } = $props();

  // A clock, only while something is in flight: the elapsed time is the whole
  // signal here — 20s on a reconcile is normal, 20s on `p4 opened` is not.
  let now = $state(Date.now());
  $effect(() => {
    if (!running.length) return;
    const t = setInterval(() => (now = Date.now()), 200);
    return () => clearInterval(t);
  });
  const secs = (r: RunningCmd) => Math.max(0, (now - r.at) / 1000);
  /** Long enough to be worth a second look, and long enough to be wrong. */
  function level(s: number): "" | "slow" | "stuck" {
    return s >= 20 ? "stuck" : s >= 3 ? "slow" : "";
  }

  // This view is in memory and capped; the file behind it keeps the whole
  // session, survives the app closing, and is what to ask someone else for when
  // their app "did nothing".
  let logPath = $state("");
  onMount(async () => {
    logPath = await p4.sessionLogPath().catch(() => "");
  });

  // Auto-scroll to the newest entry unless the user has scrolled up.
  let body: HTMLDivElement | undefined = $state();
  let stick = $state(true);
  function onScroll() {
    if (!body) return;
    stick = body.scrollTop + body.clientHeight >= body.scrollHeight - 20;
  }
  $effect(() => {
    entries.length; // track
    running.length; // a started command extends the stream too
    if (stick && body) body.scrollTop = body.scrollHeight;
  });
</script>

<div class="panel">
  <div class="hdr">
    <span class="dim">{entries.length} command{entries.length === 1 ? "" : "s"}</span>
    {#if running.length}
      <span class="runcount" title="p4 processes running right now. The app does not queue commands — everything it asks for is spawned immediately.">
        {running.length} running
      </span>
    {/if}
    {#if logPath}
      <span class="grow"></span>
      <span class="logpath dim mono" title={logPath}>{logPath.split(/[\\/]/).pop()}</span>
      <button
        title={`Every command of this session is written to:\n${logPath}\n\nThe newest 20 sessions are kept.`}
        onclick={() => revealItemInDir(logPath)}
      >
        Show log file
      </button>
    {/if}
    <button onclick={onClear} disabled={entries.length === 0}>Clear</button>
  </div>
  <div class="scroll body" bind:this={body} onscroll={onScroll}>
    {#if entries.length === 0 && !running.length}
      <div class="msg dim">No p4 commands run yet.</div>
    {:else}
      {#each entries as e (e.n)}
        <div class="row mono" class:err={!e.ok && !e.refused} class:refused={e.refused}>
          <span class="time dim">{e.time}</span>
          <span class="dot" title={e.refused ? "refused (safe mode)" : e.ok ? "ok" : "failed"}>
            {e.refused ? "⊘" : e.ok ? "●" : "✕"}
          </span>
          <span class="cmd" title={e.err ?? e.line}>{e.line}</span>
          <span class="ms dim">{e.refused ? "refused" : e.ms + "ms"}</span>
        </div>
        {#if e.err}
          <div class="errline mono">{e.err}</div>
        {/if}
      {/each}
    {/if}
    {#if running.length}
      <!-- At the END of the stream: the log reads oldest to newest downward, and a
           command still running is the newest thing there is. The view sticks to
           the bottom, so they stay in sight without being pinned. -->
      <div class="live">
        {#each running as r (r.id)}
          {@const s = secs(r)}
          <div class="row mono live-row {level(s)}">
            <span class="time dim">running</span>
            <span class="dot">◍</span>
            <span class="cmd" title={r.line}>{r.line}</span>
            <span class="ms">{s < 10 ? s.toFixed(1) : Math.round(s)}s</span>
          </div>
        {/each}
        {#if running.some((r) => secs(r) >= 20)}
          <div class="hint dim">
            Something has been running for a while. A workspace scan or a big sync
            legitimately takes tens of seconds; anything else at this age is
            probably waiting on the server or on a lock.
            <button onclick={onClearRunning}>Clear this list</button>
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-panel);
  }
  .grow {
    flex: 1;
  }
  .runcount {
    flex: none;
    font-size: 11px;
    color: var(--accent);
  }
  .live {
    border-top: 1px solid var(--border);
    background: var(--bg-alt);
  }
  .live-row .ms {
    color: var(--text-dim);
  }
  /* Age is the signal: a couple of seconds is ordinary, twenty is a question. */
  .live-row.slow .ms {
    color: var(--writable);
  }
  .live-row.stuck .ms,
  .live-row.stuck .dot {
    color: var(--warn);
  }
  .live-row .dot {
    color: var(--accent);
  }
  .hint {
    padding: 4px 10px 6px;
    font-size: 11px;
    line-height: 1.45;
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .logpath {
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 40ch;
  }
  .hdr {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--border);
    font-size: 11px;
  }
  .hdr button {
    font-size: 11px;
    padding: 2px 8px;
  }
  .body {
    flex: 1;
    padding: 4px 0;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 1px 10px;
    font-size: 12px;
    white-space: nowrap;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .time {
    flex: none;
    font-size: 11px;
  }
  .dot {
    flex: none;
    color: var(--have);
    font-size: 10px;
  }
  .row.err .dot {
    color: var(--warn);
  }
  .cmd {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row.err .cmd {
    color: var(--warn);
  }
  .row.refused .dot {
    color: var(--text-dim);
  }
  .row.refused .cmd {
    color: var(--text-dim);
    font-style: italic;
  }
  .ms {
    flex: none;
    font-size: 11px;
  }
  .msg {
    padding: 12px;
    font-size: 12px;
  }
  /* Failure text under its command — the actual p4 error, not just a red mark. */
  .errline {
    padding: 0 10px 2px 46px;
    font-size: 11px;
    color: var(--warn);
    opacity: 0.85;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
