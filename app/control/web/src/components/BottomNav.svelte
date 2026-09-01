<script>
  import { app, go } from '../lib/state.svelte.js'
  import IconHouse from '~icons/lucide/house'
  import IconShield from '~icons/lucide/shield-check'
  import IconKey from '~icons/lucide/key-round'
  import IconBook from '~icons/lucide/book-open'
  import IconSettings from '~icons/lucide/settings-2'

  // 1a 的導覽：首頁 · 白名單 · 驗證碼 · 教學 · 管理。
  // 第五個只有 admin 看得到；角色每次操作重讀 DB，降權後即時消失。
  let tabs = $derived([
    { id: 'home', label: '首頁', icon: IconHouse },
    { id: 'allow', label: '白名單', icon: IconShield },
    { id: 'codes', label: '驗證碼', icon: IconKey },
    { id: 'guide', label: '教學', icon: IconBook },
    ...(app.status?.is_admin ? [{ id: 'admin', label: '管理', icon: IconSettings }] : []),
  ])
</script>

<nav
  class="fixed bottom-0 inset-x-0 mx-auto max-w-[480px] bg-surface border-t border-line
         grid pb-[env(safe-area-inset-bottom)]"
  style="grid-template-columns: repeat({tabs.length}, 1fr)"
>
  {#each tabs as t (t.id)}
    <button
      onclick={() => go(t.id)}
      aria-current={app.tab === t.id ? 'page' : undefined}
      class="flex flex-col items-center gap-1.5 py-2.5 relative min-h-12"
    >
      <!-- 選中的分頁用實心前景色，其餘用淡化的前景 —— 靠顏色而非填滿與否
           區分，讓五個圖標在視覺重量上一致。 -->
      <t.icon
        width="21" height="21"
        stroke-width={app.tab === t.id ? 2.4 : 1.75}
        class={app.tab === t.id ? 'text-fg' : 'text-fg-faint'}
      />
      <span class="text-meta {app.tab === t.id ? 'font-semibold' : 'text-fg-muted'}">
        {t.label}
      </span>
    </button>
  {/each}
</nav>
