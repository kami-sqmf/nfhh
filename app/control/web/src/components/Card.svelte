<script>
  // 1a：卡片圓角 20。
  let { title = null, meta = null, pad = true, children } = $props()
</script>

<section class="bg-surface rounded-lg shadow-card {pad ? 'p-4' : ''}">
  {#if title}
    <header class="flex items-baseline justify-between {pad ? 'mb-3' : 'p-4 pb-0'}">
      <h2 class="text-item font-semibold">{title}</h2>
      {#if meta}<span class="text-meta text-fg-faint">{meta}</span>{/if}
    </header>
  {/if}
  {@render children()}
</section>
