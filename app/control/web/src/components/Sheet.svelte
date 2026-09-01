<script>
  // 底部彈出的確認層。1a：授權採「點一下 → 底部彈確認」，
  // 確認頁才選標籤與天數 —— 主畫面那顆按鈕保持只有一個動作。
  let { open = false, title = '', onclose, children } = $props()

  // ── 往下滑收合 ──
  // ⚠️ 只在內容捲到最頂端時才接手拖曳，否則想往下捲長內容（帳號頁有
  //    一整排 passkey）會變成把彈層拉掉 —— 最惱人的一種手勢衝突。
  let panel = $state(null)
  let dy = $state(0)
  let dragging = $state(false)
  let startY = 0
  let startedAtTop = false
  let lastY = 0
  let lastT = 0
  let vy = 0

  // 太小會誤收，太大會讓人覺得拉不動
  const DISMISS_PX = 96
  // 快速一甩就收，不必真的拉滿 96px
  const FLING_VY = 0.5 // px/ms
  const FLING_MIN_PX = 24

  function start(e) {
    startY = lastY = e.clientY
    lastT = e.timeStamp
    vy = 0
    dy = 0
    dragging = true
  }

  function down(e) {
    // 滑鼠拖曳會跟選取文字打架
    if (e.pointerType === 'mouse') return
    startedAtTop = (panel?.scrollTop ?? 0) <= 0
    start(e)
  }

  // 抓桿是專用的拖曳區：自己吃掉手勢（touch-none）並抓住 pointer，
  // 所以不管內容捲到哪裡、Safari 想不想把它當捲動，都拉得動
  function grabDown(e) {
    // 別讓它冒泡到 panel 的 down() —— 那邊會用 scrollTop 重算 startedAtTop
    e.stopPropagation()
    e.currentTarget.setPointerCapture?.(e.pointerId)
    startedAtTop = true
    start(e)
  }

  function move(e) {
    if (!dragging) return
    // 中途才捲到頂的不算，免得捲動慣性結束時突然開始拖曳
    if (!startedAtTop) return
    const dt = e.timeStamp - lastT
    if (dt > 0) {
      vy = (e.clientY - lastY) / dt
      lastY = e.clientY
      lastT = e.timeStamp
    }
    // 往上拉不動 —— 彈層已經在底部，往上只會露出背景
    dy = Math.max(0, e.clientY - startY)
  }

  function up() {
    if (!dragging) return
    dragging = false
    if (dy > DISMISS_PX || (dy > FLING_MIN_PX && vy > FLING_VY)) onclose?.()
    dy = 0
  }

  function backdrop(e) {
    if (e.target === e.currentTarget) onclose?.()
  }

  // 上次拖到一半關掉的位移不該留著
  $effect(() => { if (open) { dy = 0; dragging = false } })
</script>

<svelte:window on:keydown={(e) => open && e.key === 'Escape' && onclose?.()} />

{#if open}
  <!-- 背景可點關閉，但內容區擋掉冒泡，避免點在表單上就關掉 -->
  <div
    class="fixed inset-0 z-50 flex items-end justify-center bg-fg/45"
    onclick={backdrop}
    role="presentation"
  >
    <div
      bind:this={panel}
      onpointerdown={down}
      onpointermove={move}
      onpointerup={up}
      onpointercancel={up}
      class="w-full max-w-[480px] bg-surface rounded-t-[26px] px-5 pt-6
             pb-[calc(1.5rem+env(safe-area-inset-bottom))] max-h-[90vh] overflow-y-auto
             {dragging ? '' : 'transition-transform duration-200 ease-out'}"
      style="transform: translateY({dy}px)"
      role="dialog"
      aria-modal="true"
      aria-label={title}
    >
      <!-- 唯一在說「可以往下拉」的東西，拖曳時給一點回饋。
           外面那層是它的觸控範圍：一條 4px 的線抓不到，而且沒有
           touch-action: none 的話，Safari 一判定是捲動就送 pointercancel，
           拖曳根本不會開始 —— 就是「只有視覺、拉不動」的原因。 -->
      <div
        onpointerdown={grabDown}
        onpointermove={move}
        onpointerup={up}
        onpointercancel={up}
        role="presentation"
        class="-mt-6 -mx-5 px-5 pt-6 pb-5 touch-none select-none cursor-grab"
      >
        <div
          class="w-9 h-1 rounded-pill mx-auto transition-colors
                 {dragging ? 'bg-fg-faint' : 'bg-line-firm'}"
        ></div>
      </div>
      {#if title}<h2 class="text-title font-semibold leading-snug">{title}</h2>{/if}
      {@render children()}
    </div>
  </div>
{/if}
