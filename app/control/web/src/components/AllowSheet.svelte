<script>
  import { app, notify, fail } from '../lib/state.svelte.js'
  import { api } from '../lib/api.js'
  import Sheet from './Sheet.svelte'

  // 設計 1c。1a：授權採「點一下 → 底部彈確認」，確認頁才選標籤與天數。
  let { open = false, onclose, ondone } = $props()

  let label = $state('')
  let days = $state(7)
  let busy = $state(false)

  const s = $derived(app.status)
  const full = $derived((s?.my_entry_count ?? 0) >= (s?.max_per_user ?? 4))
  // 已經授權過的是「延長」，不佔新額度
  const extending = $derived(s?.my_ip_allowed ?? false)
  // ⚠️ `my_ip_allowed` 看的是**全部**條目（別人授權過的網路對我一樣有效），
  // 但一般成員的 `entries` 只有自己加的 —— 兩邊一比就是「這個網路通了，
  // 但不是我授權的」。後端不讓成員改寫別人的條目，這時給「延長」按鈕
  // 只會換回一句錯誤訊息，所以直接改成說明：你本來就已經能用了。
  const allowedByOther = $derived(
    !!s?.my_ip_allowed && !!s?.my_ip && !s?.entries?.some((e) => e.ip === s.my_ip),
  )
  // 授權對象應該是這個網路的公網 IPv4（見 lib/ip.js）。問不到時 my_ip 會退回
  // 連線來源，那多半是一個 IPv6 /128 —— 授權下去只保護這一台、而且會輪替，
  // 所以要說清楚，別讓人以為電視也一起通了。
  const v6 = $derived(!!s?.my_ip?.includes(':'))

  async function confirm() {
    busy = true
    try {
      // 明寫 ip：後端不帶 ip 時會退回連線來源，而那跟上面顯示的可能不是同一個
      const r = await api.allow({ ip: s?.my_ip ?? null, label: label.trim() || null, ttl_days: days })
      notify(`已授權 ${r.ip}`, true)
      label = ''
      onclose?.()
      await ondone?.()
    } catch (e) {
      fail(e)
    } finally {
      busy = false
    }
  }
</script>

<Sheet
  {open}
  {onclose}
  title={allowedByOther ? '這個網路已經通了' : extending ? '要延長這個網路嗎？' : '要授權這個網路嗎？'}
>
  <div class="mt-4 p-4 rounded-md bg-canvas">
    <div class="font-mono text-head font-medium tracking-tight">{s?.my_ip ?? '取不到 IP'}</div>
    <div class="mt-1.5 text-body text-fg-muted">
      公網位址 · 你的額度 <span class="font-mono">{s?.my_entry_count} / {s?.max_per_user}</span>
    </div>
  </div>

  {#if allowedByOther}
    <div class="mt-3 p-4 rounded-md bg-ok-bg text-body leading-relaxed text-ok-fg text-pretty">
      這個網路已由家中其他成員授權，你已經可以直接使用。
    </div>

    <div class="mt-5">
      <button onclick={onclose} class="w-full py-4 rounded-lg text-item font-medium text-fg-muted">
        關閉
      </button>
    </div>
  {:else}
    {#if v6}
      <div class="mt-3 p-4 rounded-md bg-watch-bg text-body leading-relaxed text-watch-fg text-pretty">
        取不到這個網路的對外 IPv4，只好用目前這個 IPv6 位址。它只涵蓋這一台裝置，
        而且位址會定期換掉 —— 電視那類走 IPv4 的裝置不會因此被放行。
        請改用同一個網路的其他瀏覽器再試一次。
      </div>
    {/if}

    {#if full && !extending}
      <div class="mt-3 p-4 rounded-md bg-bad-bg text-body leading-relaxed text-bad-fg text-pretty">
        你的額度已經滿了。到「白名單」移除不再用的網路，才能授權新的。
      </div>
    {/if}

    <div class="mt-5 text-body font-semibold text-fg-strong">名稱（選填）</div>
    <input
      bind:value={label}
      placeholder="例如：咖啡廳、飯店 Wi-Fi"
      class="mt-2 w-full px-4 py-3.5 rounded-sm border-[1.5px] border-line-firm bg-transparent
             outline-none focus:border-fg"
    />

    <div class="mt-4 flex items-baseline justify-between">
      <span class="text-body font-semibold text-fg-strong">自動續期天數</span>
      <span class="text-label text-fg-faint">上限 30 天</span>
    </div>
    <div class="mt-2 flex gap-2">
      {#each [1, 7, 30] as d (d)}
        <button
          onclick={() => (days = d)}
          class="flex-1 py-3 rounded-sm font-mono text-lead
                 {days === d ? 'bg-ok text-white font-medium' : 'border-[1.5px] border-line-firm text-fg-muted'}"
        >{d} 天</button>
      {/each}
    </div>

    <p class="mt-4 text-body leading-relaxed text-fg-faint text-pretty">
      若這個網路上仍有裝置在查詢，到期前會自動續期。完全沒有查詢的會照常到期並移除。
    </p>

    <div class="mt-5 flex flex-col gap-2.5">
      <button
        onclick={confirm}
        disabled={busy || (full && !extending)}
        class="w-full py-5 rounded-lg bg-ok text-white text-lead font-semibold disabled:opacity-40"
      >{extending ? '確認延長' : '確認授權'}</button>
      <button onclick={onclose} class="w-full py-4 rounded-lg text-item font-medium text-fg-muted">
        取消
      </button>
    </div>
  {/if}
</Sheet>
