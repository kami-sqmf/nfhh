<script>
  import { notify } from '../lib/state.svelte.js'
  let { value, label = null, hint = '複製' } = $props()

  async function copy() {
    try {
      await navigator.clipboard.writeText(value)
      notify(`已複製 ${value}`, true)
    } catch {
      // iOS 在非 https 或某些 webview 下沒有 clipboard API
      notify('請長按上方文字手動複製')
    }
  }
</script>

<button
  onclick={copy}
  class="w-full flex items-center justify-between gap-3 px-4 py-3.5
         rounded-md bg-canvas text-left"
>
  <span class="min-w-0">
    {#if label}
      <span class="block font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
        {label}
      </span>
    {/if}
    <span class="block font-mono text-lead font-medium truncate">{value}</span>
  </span>
  <span class="text-body font-medium text-ok shrink-0">{hint}</span>
</button>
