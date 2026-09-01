<script>
  // 關鍵字／排除字／白名單網域共用（設計 1g、1m 各出現一次）。
  let { tags = [], tone = 'none', placeholder = '新增', onchange } = $props()

  let adding = $state(false)
  let draft = $state('')
  let input = $state(null)

  const tones = {
    ok: 'bg-ok-bg text-ok-fg',
    bad: 'bg-bad-bg text-bad-fg',
    none: 'bg-canvas text-fg-strong',
  }

  function start() {
    adding = true
    draft = ''
    // 等 DOM 更新後才對得到那個 input
    queueMicrotask(() => input?.focus())
  }

  function commit() {
    const v = draft.trim().toLowerCase()
    // 去重而不是報錯 —— 重複輸入的意圖顯然是「要有這一個」
    if (v && !tags.includes(v)) onchange?.([...tags, v])
    adding = false
    draft = ''
  }

  function remove(t) {
    onchange?.(tags.filter((x) => x !== t))
  }
</script>

<div class="flex flex-wrap gap-2 items-center">
  {#each tags as t (t)}
    <span class="flex items-center gap-2 text-label font-medium px-3 py-1.5 rounded-pill {tones[tone]}">
      {t}
      <button onclick={() => remove(t)} aria-label="移除 {t}" class="opacity-50 min-h-0 leading-none">✕</button>
    </span>
  {/each}

  {#if adding}
    <input
      bind:this={input}
      bind:value={draft}
      onblur={commit}
      onkeydown={(e) => {
        if (e.key === 'Enter') commit()
        if (e.key === 'Escape') adding = false
      }}
      {placeholder}
      autocapitalize="off"
      spellcheck="false"
      class="px-3 py-1.5 rounded-pill border-[1.5px] border-line-firm bg-transparent
             text-label w-40 outline-none focus:border-ok"
    />
  {:else}
    <button
      onclick={start}
      class="text-label font-medium px-3 py-1.5 rounded-pill min-h-0
             border-[1.5px] border-dashed border-line-firm text-fg-muted"
    >
      ＋ {placeholder}
    </button>
  {/if}
</div>
