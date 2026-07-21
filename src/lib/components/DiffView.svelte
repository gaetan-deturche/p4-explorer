<script lang="ts">
  let { text }: { text: string } = $props();

  type DRow = {
    ln: number | null;
    lt: string | null;
    rn: number | null;
    rt: string | null;
    cls: "ctx" | "del" | "add" | "chg";
  };

  // Transform a `p4 diff2 -u` unified diff into aligned side-by-side rows.
  function sideBySide(u: string): DRow[] {
    const rows: DRow[] = [];
    let oldLn = 0;
    let newLn = 0;
    let dels: { n: number; t: string }[] = [];
    let adds: { n: number; t: string }[] = [];
    const flush = () => {
      const m = Math.max(dels.length, adds.length);
      for (let i = 0; i < m; i++) {
        const d = dels[i];
        const a = adds[i];
        rows.push({
          ln: d ? d.n : null,
          lt: d ? d.t : null,
          rn: a ? a.n : null,
          rt: a ? a.t : null,
          cls: d && a ? "chg" : d ? "del" : "add",
        });
      }
      dels = [];
      adds = [];
    };
    for (const line of u.split("\n")) {
      if (!line) continue;
      if (line.startsWith("@@")) {
        flush();
        const m = /@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
        if (m) {
          oldLn = +m[1];
          newLn = +m[2];
        }
        continue;
      }
      if (line.startsWith("---") || line.startsWith("+++")) continue;
      if (line.startsWith("-")) {
        dels.push({ n: oldLn++, t: line.slice(1) });
        continue;
      }
      if (line.startsWith("+")) {
        adds.push({ n: newLn++, t: line.slice(1) });
        continue;
      }
      flush();
      const t = line.startsWith(" ") ? line.slice(1) : line;
      rows.push({ ln: oldLn++, lt: t, rn: newLn++, rt: t, cls: "ctx" });
    }
    flush();
    return rows;
  }

  const rows = $derived(sideBySide(text));

  const lcls = (r: DRow): string =>
    r.cls === "del" || r.cls === "chg" ? "del" : r.cls === "add" ? "gap" : "ctx";
  const rcls = (r: DRow): string =>
    r.cls === "add" || r.cls === "chg" ? "add" : r.cls === "del" ? "gap" : "ctx";
</script>

<!-- One grid, one scrollbar: rows align automatically and vertical scrolling is
     native (no JS sync, no lag). Long lines wrap rather than scroll sideways. -->
<div class="diff2">
  {#each rows as r, i (i)}
    <div class="num {lcls(r)}">{r.ln ?? ""}</div>
    <div class="code {lcls(r)}">{r.lt ?? ""}</div>
    <div class="num sep {rcls(r)}">{r.rn ?? ""}</div>
    <div class="code {rcls(r)}">{r.rt ?? ""}</div>
  {/each}
</div>

<style>
  .diff2 {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto minmax(0, 1fr);
    max-height: 40vh;
    overflow-y: auto;
    overflow-x: hidden;
    background: var(--bg-alt);
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    font-family: var(--mono);
    font-size: 11px;
    line-height: 1.5;
  }
  .num {
    text-align: right;
    padding: 0 6px;
    color: var(--text-dim);
    -webkit-user-select: none;
    user-select: none;
    white-space: nowrap;
  }
  .num.sep {
    border-left: 1px solid var(--border);
  }
  .code {
    padding: 0 8px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    word-break: break-word;
  }
  .del {
    background: rgba(192, 57, 43, 0.16);
    color: var(--warn);
  }
  .add {
    background: rgba(31, 157, 85, 0.16);
    color: var(--have);
  }
  .gap {
    background: rgba(128, 128, 128, 0.07);
  }
</style>
