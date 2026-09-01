<script>
  import { app, notify, fail } from '../../lib/state.svelte.js'
  import { api } from '../../lib/api.js'
  import { stamp } from '../../lib/time.js'
  import SubHeader from '../../components/SubHeader.svelte'
  import Pill from '../../components/Pill.svelte'
  import PlatformMark from '../../components/PlatformMark.svelte'
  import MailView from '../../components/MailView.svelte'

  // 設計 1n。不做平台過濾也不做驗證過濾 —— 這是 admin 診斷
  // 「為什麼某封信沒出現在驗證碼分頁」的地方，看得到全部才有用。
  let mails = $state([])
  let filter = $state('all')
  let viewing = $state(null)

  const platform = (c) => app.status?.platforms?.find((p) => p.code === c)
  const platformName = (c) => platform(c)?.name ?? (c ?? '未知')

  // 「未命中」跟「未通過」是兩件事：前者是抽不到驗證碼（廣告信），
  // 後者是抽到了但寄件者沒過驗證。混在一起會讓診斷失去意義。
  const kind = (m) =>
    !m.code ? 'miss' : m.verified === true ? 'ok' : m.verified === false ? 'fail' : 'unknown'

  const counts = $derived({
    all: mails.length,
    ok: mails.filter((m) => kind(m) === 'ok').length,
    fail: mails.filter((m) => kind(m) === 'fail').length,
    miss: mails.filter((m) => kind(m) === 'miss').length,
    unknown: mails.filter((m) => kind(m) === 'unknown').length,
  })

  // 「無驗證資訊」是 v5 之前的舊信，正常情況下是 0。有值才顯示這個籤 ——
  // 否則四個數字加不起來，會讓人以為有信件不見了。
  const chips = $derived([
    ['all', '全部'], ['ok', '已驗證'], ['fail', '未通過'], ['miss', '未命中'],
    ...(counts.unknown > 0 ? [['unknown', '無驗證資訊']] : []),
  ])

  const shown = $derived(filter === 'all' ? mails : mails.filter((m) => kind(m) === filter))

  const badge = {
    ok: { tone: 'ok', text: '✔ 已認證' },
    fail: { tone: 'watch', text: '未通過' },
    miss: { tone: 'none', text: '未命中' },
    unknown: { tone: 'none', text: '無驗證資訊' },
  }

  async function load() {
    try { mails = await api.inbox() } catch (e) { fail(e) }
  }

  async function del(id) {
    try { await api.deleteMail(id); mails = mails.filter((m) => m.id !== id) } catch (e) { fail(e) }
  }

  async function purge() {
    if (!confirm(`刪除全部 ${mails.length} 封信？這會清掉所有人的驗證碼，無法復原。`)) return
    try {
      const r = await api.purgeMails()
      notify(`已刪除 ${r.deleted} 封`, true)
      mails = []
    } catch (e) { fail(e) }
  }

  $effect(() => { load() })
</script>

<SubHeader title="收件匣">
  <button onclick={purge} class="absolute right-5 top-5 min-h-0 text-label font-medium text-bad-fg">
    全部刪除
  </button>
</SubHeader>

<div class="px-5 pt-3 flex flex-col gap-2">
  <div class="flex gap-1.5 overflow-x-auto pb-1">
    {#each chips as [id, label] (id)}
      <button
        onclick={() => (filter = id)}
        class="px-3 py-1.5 min-h-0 rounded-pill text-label whitespace-nowrap
               {filter === id ? 'bg-fg text-canvas font-semibold' : 'bg-surface text-fg-muted'}"
      >{label} {counts[id]}</button>
    {/each}
  </div>

  {#each shown as m (m.id)}
    <div class="bg-surface rounded-md p-3.5">
      <div class="flex items-center justify-between gap-2">
        <div class="flex items-center gap-2 min-w-0">
          <PlatformMark code={m.platform ?? ""} color={platform(m.platform)?.color} name={platformName(m.platform)} size="sm" />
          <span class="text-body font-semibold truncate">{platformName(m.platform)}</span>
          <span class="font-mono text-micro text-fg-faint shrink-0">{stamp(m.received_at)}</span>
        </div>
        <Pill tone={badge[kind(m)].tone}>{badge[kind(m)].text}</Pill>
      </div>

      <div class="mt-1.5 flex items-end justify-between gap-3">
        <div class="min-w-0">
          <div class="text-label text-fg-muted truncate">{m.subject || '(無主旨)'}</div>
          {#if m.code}
            <div class="mt-1 font-mono text-title font-semibold leading-none tracking-[0.05em]">
              {m.code}
            </div>
          {:else}
            <div class="mt-1 text-label text-fg-faint">{m.skip_reason ?? '未擷取到驗證碼'}</div>
          {/if}
        </div>
        <div class="flex gap-1.5 shrink-0">
          <button
            onclick={() => (viewing = m)}
            class="px-2.5 py-2 min-h-0 rounded-sm border-[1.5px] border-line-firm text-label font-medium"
          >原始信件</button>
          <button
            onclick={() => del(m.id)}
            class="px-2.5 py-2 min-h-0 rounded-sm border-[1.5px] border-bad/35 text-label font-medium text-bad-fg"
          >刪除</button>
        </div>
      </div>

      {#if m.verified === false}
        <div class="mt-2 pt-1.5 border-t border-line font-mono text-micro leading-relaxed text-watch-fg">
          寄件者未通過驗證 · 依目前的顯示策略處理
        </div>
      {:else if m.platform === null && m.code}
        <div class="mt-2 pt-1.5 border-t border-line text-micro leading-relaxed text-fg-faint">
          認不出平台，因此不會出現在任何人的驗證碼分頁。
          檢查收件信箱的名稱是否對應某個 domain-set。
        </div>
      {/if}
    </div>
  {:else}
    <p class="bg-surface rounded-md p-5 text-body text-fg-faint">這個篩選沒有符合的信件。</p>
  {/each}
</div>

<MailView mail={viewing} onclose={() => (viewing = null)} />
