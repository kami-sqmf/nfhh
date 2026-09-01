<script>
  import { app, go, fail } from '../lib/state.svelte.js'
  import { api } from '../lib/api.js'
  import { stamp } from '../lib/time.js'
  import IconInbox from '~icons/lucide/inbox'
  import IconUsers from '~icons/lucide/users'
  import IconForward from '~icons/lucide/forward'
  import IconBadge from '~icons/lucide/badge-check'

  // 設計 1h。第五個分頁只有 admin 看得到，裡面是家人不需要、
  // 但管理員必須看得出漂移的資料表。
  let audit = $state([])
  let showAll = $state(false)

  const tiles = [
    { id: 'inbox', label: '收件匣', icon: IconInbox },
    { id: 'members', label: '成員管理', icon: IconUsers },
    { id: 'recipients', label: '轉發收件人', icon: IconForward },
    { id: 'sender', label: '寄件者驗證', icon: IconBadge },
  ]

  // 動作分三類上色：成功建立／一般紀錄／需要注意。
  // 顏色的意義跟別處一致 —— 琥珀是「還在觀察」，不是「壞了」。
  function tone(action) {
    if (action.includes('bad') || action.includes('denied') || action.includes('locked'))
      return 'bg-bad-bg text-bad-fg'
    if (action.includes('unverified') || action.includes('not_invited'))
      return 'bg-watch-bg text-watch-fg'
    if (action.includes('add') || action.includes('created') || action.includes('granted'))
      return 'bg-ok-bg text-ok-fg'
    return 'bg-wash text-fg-muted'
  }

  $effect(() => {
    api.audit().then((r) => (audit = r)).catch(fail)
  })

  const shown = $derived(showAll ? audit : audit.slice(0, 3))
</script>

<header class="px-5 pt-5 pb-3.5 bg-surface">
  <h1 class="text-title font-bold">管理首頁</h1>
</header>

<div class="px-5 pt-3 flex flex-col gap-2.5">
  <div class="grid grid-cols-2 gap-2">
    {#each tiles as t (t.id)}
      <button
        onclick={() => go('admin', t.id)}
        class="py-4 px-3 bg-surface rounded-md flex flex-col items-center gap-2"
      >
        <t.icon width="22" height="22" stroke-width="1.75" class="text-fg" />
        <span class="text-label font-semibold">{t.label}</span>
      </button>
    {/each}
  </div>

  <div class="bg-surface rounded-md p-4">
    <div class="flex items-baseline justify-between">
      <h2 class="text-body font-semibold">稽核紀錄</h2>
      <span class="text-meta text-fg-faint">最近 {audit.length} 筆</span>
    </div>

    <div class="mt-1 divide-y divide-line">
      <!-- key 一律用真的身分。這裡曾經拿 at + action + detail 拼 key，
           同一秒兩個人做同一件事就撞，Svelte 丟 each_key_duplicate，
           整塊畫不出來 —— 而且是在 flush 裡爆的，看起來像按鈕沒反應。 -->
      {#each shown as r (r.id)}
        <div class="py-2.5">
          <div class="flex items-center gap-2">
            <span class="font-mono text-micro font-medium px-2 py-1 rounded-chip {tone(r.action)}">
              {r.action}
            </span>
            <span class="font-mono text-meta text-fg-faint">{stamp(r.at)}</span>
          </div>
          <div class="mt-1.5 text-label leading-relaxed text-fg-strong">
            {#if r.actor}<span class="font-mono">{r.actor}</span>{/if}
            {r.detail ?? ''}
          </div>
        </div>
      {:else}
        <p class="py-3 text-body text-fg-faint">尚無紀錄</p>
      {/each}
    </div>

    {#if audit.length > 3}
      <button
        onclick={() => (showAll = !showAll)}
        class="mt-1.5 w-full py-2.5 rounded-sm border-[1.5px] border-line-firm text-label
               font-medium active:opacity-60"
      >
        {showAll ? '收合' : `展開更多（還有 ${audit.length - 3} 筆）`}
      </button>
    {/if}
  </div>
</div>
