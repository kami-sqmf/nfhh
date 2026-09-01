<script>
  import { go } from '../lib/state.svelte.js'
  let { title, sub = null, children } = $props()
</script>

<header class="relative px-5 pt-5 pb-4 bg-surface">
  <div class="flex items-center gap-2 text-label text-fg-faint">
    <button onclick={() => go('admin')} class="min-h-0 text-ok">管理</button>
    <span>/</span>
    <span>{title}</span>
  </div>
  <h1 class="mt-1.5 text-head font-bold">{title}</h1>
  {#if sub}<p class="mt-1 text-body text-fg-faint">{sub}</p>{/if}
  {#if children}{@render children()}{/if}
</header>
