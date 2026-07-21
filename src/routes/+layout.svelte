<script lang="ts">
  import "../app.css";
  let { children } = $props();
</script>

<!-- Suppress the webview's native context menu so right-click can be handled
     per-region with custom menus. Individual elements add their own handlers. -->
<svelte:window oncontextmenu={(e) => e.preventDefault()} />

{@render children?.()}
