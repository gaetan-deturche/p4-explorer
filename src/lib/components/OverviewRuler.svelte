<script lang="ts">
  //! A marker strip beside the scrollbar: one tick per change, click to jump.
  //!
  //! Ticks sit at their fraction of the document, so the strip reads as a map of
  //! where the work is — including the parts scrolled out of sight, which is the
  //! whole point of it.

  export interface Mark {
    /** 0..1 position in the document. */
    pct: number;
    /** Tick colour, matching the panes: add | del | mod | conflict | done. */
    kind: "add" | "del" | "mod" | "conflict" | "done";
    title: string;
    /** Passed back on click so the host can scroll to the right thing. */
    index: number;
  }

  let {
    marks,
    offsetRight = 0,
    onPick,
    onSeek,
  }: {
    marks: Mark[];
    /** Leaves room for the native scrollbar, whose width the host measures. */
    offsetRight?: number;
    onPick: (index: number) => void;
    onSeek: (fraction: number) => void;
  } = $props();

  let strip: HTMLDivElement | undefined = $state();

  function seek(e: MouseEvent) {
    if (!strip) return;
    const box = strip.getBoundingClientRect();
    onSeek(Math.max(0, Math.min(1, (e.clientY - box.top) / box.height)));
  }
</script>

<div
  class="ruler"
  bind:this={strip}
  style="right:{offsetRight}px"
  role="button"
  tabindex="-1"
  aria-label="Jump to a change"
  title="Changes in this file — click to jump"
  onclick={seek}
  onkeydown={(e) => {
    if ((e.key === "Enter" || e.key === " ") && marks.length) {
      e.preventDefault();
      onPick(marks[0].index);
    }
  }}
>
  {#each marks as m (m.index)}
    <button
      class="tick {m.kind}"
      style="top:{m.pct * 100}%"
      title={m.title}
      onclick={(e) => {
        e.stopPropagation();
        onPick(m.index);
      }}
    ></button>
  {/each}
</div>

<style>
  .ruler {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 10px;
    background: rgba(255, 255, 255, 0.03);
    border-left: 1px solid var(--border, #333);
    cursor: pointer;
    z-index: 3;
  }
  .tick {
    position: absolute;
    left: 1px;
    right: 1px;
    height: 3px;
    padding: 0;
    border: 0;
    border-radius: 1px;
    cursor: pointer;
    transform: translateY(-1px);
  }
  .tick:hover {
    height: 5px;
    left: 0;
    right: 0;
  }
  /* The same scale as the panes: green is kept, orange is dropped, red needs a
     decision. A block changed on both sides shows both halves. */
  .tick.add {
    background: #5faf5f;
  }
  .tick.del {
    background: #d9873a;
  }
  .tick.mod {
    background: linear-gradient(to right, #d9873a 0 50%, #5faf5f 50% 100%);
  }
  .tick.conflict {
    background: #e0555a;
  }
  .tick.done {
    background: #5faf5f;
  }
</style>
