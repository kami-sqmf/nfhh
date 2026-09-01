<script>
  // 六格驗證碼輸入（設計 1o）。
  //
  // 底層是**一個** input 而不是六個：六個各自帶焦點管理的輸入框在
  // iOS 上會跟自動填入打架，而且退格要跨格移動、貼上要拆字，
  // 每一項都是 bug 的來源。這裡用一個透明的 input 疊在六個格子上，
  // 自動填入、貼上、退格全部是瀏覽器原生行為。
  let { value = $bindable(''), disabled = false, oncomplete } = $props()

  let cells = $derived([...Array(6)].map((_, i) => value[i] ?? ''))

  function oninput(e) {
    value = e.target.value.replace(/\D/g, '').slice(0, 6)
    if (value.length === 6) oncomplete?.(value)
  }
</script>

<div class="relative">
  <input
    class="absolute inset-0 w-full h-full opacity-0 tracking-[3em]"
    type="text"
    inputmode="numeric"
    autocomplete="one-time-code"
    maxlength="6"
    {disabled}
    {value}
    {oninput}
    aria-label="六位數驗證碼"
  />
  <div class="flex gap-2 pointer-events-none">
    {#each cells as c, i (i)}
      <div
        class="flex-1 py-4 text-center bg-surface rounded-md font-mono text-head font-medium
               {i === value.length && !disabled ? 'ring-2 ring-fg' : ''}
               {c ? '' : 'text-fg-faint/40'}"
      >
        {c || '_'}
      </div>
    {/each}
  </div>
</div>
