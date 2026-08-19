<script lang="ts">
  //! Who has a file open, and who is blocking everyone else.
  //!
  //! The distinction is the whole point of the dialog: on an exclusive-open type
  //! (`+l`) a single checkout locks the file for everyone, while on an ordinary
  //! type any number of people can hold it at once and nobody is blocked. p4
  //! reports those two situations with the same fields, so the wording here has
  //! to make them different.
  import type { FileHolders } from "$lib/p4";

  let {
    info,
    loading,
    error,
    me,
    onClose,
  }: {
    info: FileHolders | null;
    loading: boolean;
    error: string;
    me: string;
    onClose: () => void;
  } = $props();

  const name = $derived((info?.depotFile ?? "").split("/").pop() || "file");
  /** Who is actually in the way: an explicit lock holder, or the one open on a
   *  `+l` file. Empty when the file is shared or free. */
  const blocker = $derived(info?.others.find((o) => o.blocking) ?? null);
  const heldByMe = $derived(!!info?.ourAction || !!info?.ourLock);
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onClose()} />

<div class="overlay">
  <button class="backdrop" aria-label="Close" onclick={onClose}></button>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    <div class="dtitle mono">{name}</div>

    {#if loading}
      <p class="msg dim">Asking the server…</p>
    {:else if error}
      <p class="msg bad">{error}</p>
    {:else if info}
      <p class="msg">
        {#if blocker}
          <strong>{blocker.user}</strong> holds this file
          {info.otherLock ? "locked" : "exclusively"} — nobody else can open it until they
          submit or revert.
        {:else if info.ourLock}
          You hold an explicit lock on this file.
        {:else if info.exclusiveType && info.ourAction}
          You have it open, and its type is exclusive — nobody else can open it while you do.
        {:else if info.others.length}
          {info.others.length} other{info.others.length === 1 ? "" : "s"} have this open. Its type
          is not exclusive, so that blocks nobody.
        {:else if info.ourAction}
          Only you have this open.
        {:else}
          Nobody has this file open.
        {/if}
      </p>

      {#if info.others.length}
        <table>
          <tbody>
            {#each info.others as o (o.user + o.client + o.change)}
              <tr class:blocking={o.blocking}>
                <td class="who">{o.user}{o.user === me ? " (you, elsewhere)" : ""}</td>
                <td class="act">{o.action}</td>
                <td class="chg mono">{o.change === "default" ? "default" : "@" + o.change}</td>
                <td class="cl mono dim" title={o.client}>{o.client}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}

      <p class="msg dim small">
        <span class="mono">{info.headType || "unknown type"}</span>
        {info.exclusiveType ? " — exclusive open (+l): one checkout at a time" : " — shared: several people may open it at once"}
        {#if heldByMe && info.ourAction}
          · you have it open for {info.ourAction}
        {/if}
      </p>
    {/if}

    <div class="actions"><button class="primary" onclick={onClose}>Close</button></div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }
  .backdrop {
    position: absolute;
    inset: 0;
    border: 0;
    padding: 0;
    background: rgba(0, 0, 0, 0.45);
  }
  .dialog {
    position: relative;
    min-width: 420px;
    max-width: 70vw;
    padding: 14px 16px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 10px 34px rgba(0, 0, 0, 0.45);
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
    margin-bottom: 8px;
    overflow-wrap: anywhere;
  }
  .msg {
    margin: 0 0 10px;
    font-size: 12px;
    line-height: 1.45;
  }
  .small {
    font-size: 11px;
  }
  .dim {
    opacity: 0.7;
  }
  .bad {
    color: #f08a8a;
  }
  .mono {
    font-family: var(--mono);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    margin: 0 0 10px;
    font-size: 12px;
  }
  td {
    padding: 2px 6px 2px 0;
    white-space: nowrap;
  }
  .blocking .who {
    font-weight: 600;
    color: var(--accent);
  }
  .cl {
    max-width: 22ch;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
