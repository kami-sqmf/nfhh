<script>
  import { app, notify, fail, refresh } from '../lib/state.svelte.js'
  import { api } from '../lib/api.js'
  import { left, expiringSoon, stamp } from '../lib/time.js'
  import Pill from '../components/Pill.svelte'
  import AllowSheet from '../components/AllowSheet.svelte'

  // 設計 1d。一般成員只看得到自己新增的 IP（後端已經過濾），
  // admin 看得到全部但**拿不到別人的查詢明細** —— 那份只有擁有者能展開。
  let authorize = $state(false)
  let expanded = $state(null)
  let queries = $state([])
  let renaming = $state(null)
  let draft = $state('')

  const s = $derived(app.status)

  async function toggle(ip) {
    if (expanded === ip) return (expanded = null)
    expanded = ip
    queries = []
    try { queries = await api.queries(ip) } catch (e) { fail(e) }
  }

  async function remove(ip) {
    if (!confirm(`移除 ${ip}？這個網路上的裝置會立刻失去存取。`)) return
    try {
      await api.unallow(ip)
      notify(`已移除 ${ip}`, true)
      await refresh()
    } catch (e) { fail(e) }
  }

  async function rename(ip) {
    try {
      await api.rename(ip, draft.trim() || null)
      renaming = null
      await refresh()
    } catch (e) { fail(e) }
  }
</script>

<header class="px-5 pt-5 pb-4 bg-surface">
  <h1 class="text-head font-bold">白名單</h1>

  <div class="mt-3 flex items-center justify-between gap-3 px-3.5 py-3 rounded-sm bg-canvas">
    <span class="min-w-0">
      <span class="block font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
        目前出口 IP
      </span>
      <span class="block mt-0.5 font-mono text-title font-medium truncate">{s?.my_ip ?? '取不到'}</span>
    </span>
    <Pill tone={s?.my_ip_allowed ? 'ok' : 'bad'}>
      {s?.my_ip_allowed ? '已授權' : '未授權'}
    </Pill>
  </div>

  <button
    onclick={() => (authorize = true)}
    class="mt-2.5 w-full py-3.5 rounded-sm bg-ok text-white text-lead font-semibold"
  >
    {s?.my_ip_allowed ? '延長授權期限' : '授權這個網路'}
  </button>
</header>

<div class="px-5 pt-3 flex flex-col gap-2">
  <div class="flex items-baseline justify-between px-1">
    <span class="font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
      {s?.is_admin ? '全部授權' : '你新增的 IP'}
    </span>
    <span class="text-label text-fg-faint">
      額度 <span class="font-mono text-fg">{s?.my_entry_count} / {s?.max_per_user}</span>
    </span>
  </div>

  {#each s?.entries ?? [] as e (e.ip)}
    <div class="bg-surface rounded-md p-4">
      <div class="flex items-start justify-between gap-3">
        <div class="min-w-0">
          <div class="font-mono text-lead font-medium truncate">{e.ip}</div>
          <div class="mt-1 text-label text-fg-faint truncate">
            {e.label || '未命名'} ·
            <span class="font-mono {expiringSoon(e.expires_at) ? 'text-watch-fg' : ''}">
              剩 {left(e.expires_at)}
            </span>
            {#if e.renewed_at}<span class="text-fg-faint"> · 已自動續期</span>{/if}
          </div>
        </div>
        {#if e.ip === s?.my_ip}
          <Pill tone="ok">目前所在</Pill>
        {/if}
      </div>

      <!-- 活躍度。彙總數字人人可見；逐筆網域只有擁有者展得開。 -->
      <div class="mt-2.5 pt-2.5 border-t border-line">
        {#if e.queries.count > 0}
          <button
            onclick={() => e.mine && toggle(e.ip)}
            class="flex items-center gap-2 text-meta text-fg-muted min-h-0 {e.mine ? '' : 'cursor-default'}"
          >
            <span class="w-1.5 h-1.5 rounded-full bg-ok shrink-0"></span>
            <span>近 5 分鐘 <span class="font-mono text-fg">{e.queries.count}</span> 筆查詢</span>
            {#if e.mine}
              <span class="font-medium text-ok">{expanded === e.ip ? '收合' : '展開'}</span>
            {/if}
          </button>
        {:else}
          <div class="flex items-center gap-2 text-meta text-fg-faint">
            <span class="w-1.5 h-1.5 rounded-full bg-line-firm shrink-0"></span>
            <span>近 5 分鐘無查詢 · 到期後自動移除</span>
          </div>
        {/if}

        {#if expanded === e.ip}
          <div class="mt-2 flex flex-col gap-1.5 font-mono text-micro text-fg-muted">
            <!-- key 用 seq，不要拿 (at, domain) 拼：同一個網域的 A 與 AAAA
                 查詢會在同一秒各記一筆，那兩列從欄位上完全分不出來。 -->
            {#each queries as q (q.seq)}
              <div class="flex justify-between gap-3">
                <span class="shrink-0">{stamp(q.at)}</span>
                <span class="truncate">{q.domain}</span>
              </div>
            {:else}
              <span class="text-fg-faint">視窗內沒有明細（面板重啟後會重新累積）</span>
            {/each}
          </div>
        {/if}
      </div>

      {#if renaming === e.ip}
        <div class="mt-2.5 flex gap-2">
          <input
            bind:value={draft}
            placeholder="例如：咖啡廳"
            onkeydown={(k) => k.key === 'Enter' && rename(e.ip)}
            class="flex-1 min-w-0 px-3 py-2.5 rounded-sm border-[1.5px] border-line-firm
                   bg-transparent text-body outline-none focus:border-fg"
          />
          <button onclick={() => rename(e.ip)} class="px-4 py-2.5 min-h-0 rounded-sm bg-fg text-canvas text-body font-medium">儲存</button>
          <button onclick={() => (renaming = null)} class="px-3 py-2.5 min-h-0 text-body text-fg-muted">取消</button>
        </div>
      {:else if e.mine || s?.is_admin}
        <div class="mt-2.5 flex gap-2">
          <button
            onclick={() => { renaming = e.ip; draft = e.label ?? '' }}
            class="flex-1 py-2.5 min-h-0 rounded-sm border-[1.5px] border-line-firm text-body font-medium"
          >重新命名</button>
          <button
            onclick={() => remove(e.ip)}
            class="flex-1 py-2.5 min-h-0 rounded-sm border-[1.5px] border-bad/35 text-body font-medium text-bad-fg"
          >移除</button>
        </div>
      {/if}
    </div>
  {:else}
    <div class="bg-surface rounded-md p-5 text-body leading-relaxed text-fg-muted text-pretty">
      還沒有授權任何網路。多數情況下你不需要 —— 只有電視出現同戶裝置限制時才用得到。
    </div>
  {/each}
</div>

<AllowSheet open={authorize} onclose={() => (authorize = false)} ondone={refresh} />
