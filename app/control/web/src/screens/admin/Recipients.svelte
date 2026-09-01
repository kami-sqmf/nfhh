<script>
  import { app, notify, fail } from '../../lib/state.svelte.js'
  import { api } from '../../lib/api.js'
  import { since } from '../../lib/time.js'
  import SubHeader from '../../components/SubHeader.svelte'
  import Toggle from '../../components/Toggle.svelte'
  import PlatformMark from '../../components/PlatformMark.svelte'

  // 設計 1g。Worker 轉發前會來面板問這份清單。
  //
  // ⚠️ 分組照 admin 明說的「平台 → 收件信箱」對應，**不是**用 `代號@網域`
  //    推的 —— 那對 Disney+ 是錯的（代號 disneyplus、信箱 disney@），
  //    推錯會把家人加到一個收不到信的信箱，而且沒有任何徵兆。
  let list = $state([])
  let boxes = $state({})
  let defaultDomain = $state('')
  let busy = $state(false)
  let adding = $state(null) // 正在新增到哪個 mailbox
  let draftAddr = $state('')
  let draftLabel = $state('')
  let editing = $state(null) // 正在設定哪個平台的信箱
  let draftMailbox = $state('')

  const cfEnabled = $derived(app.status?.cf_enabled ?? false)
  const platforms = $derived(app.status?.platforms ?? [])

  // 四種狀態，處理方式各不相同，混用任兩種都會讓 admin 走錯方向：
  //   未查詢    沒設 token 或還沒查過
  //   未登記    Cloudflare 沒有這個位址 —— 轉發一定退信，信也從沒寄過
  //   尚未驗證  位址在，信寄了，對方沒點
  //   已驗證    正常
  function status(r) {
    if (!cfEnabled || r.cf_checked_at == null) return { tone: 'none', text: '未查詢' }
    if (r.cf_present === false) return { tone: 'bad', text: '未登記到 Cloudflare', fix: true }
    if (r.cf_verified_at) return { tone: 'ok', text: `已驗證・${since(r.cf_verified_at)}` }
    return { tone: 'bad', text: '尚未驗證', fix: true }
  }

  const rowsOf = (mailbox) => list.filter((r) => r.mailbox === mailbox)

  // 有收件人卻不屬於任何平台的信箱 —— 收不到任何信，卻看起來像有效的目標
  const orphans = $derived([
    ...new Set(list.map((r) => r.mailbox).filter((m) => !Object.values(boxes).includes(m))),
  ])

  async function load() {
    try {
      const r = await api.recipients()
      list = r.recipients
      boxes = r.mailboxes ?? {}
      defaultDomain = r.default_domain ?? ''
    } catch (e) { fail(e) }
  }

  async function saveMailbox(code) {
    busy = true
    try {
      await api.setMailbox(code, draftMailbox.trim())
      notify(draftMailbox.trim() ? '已設定收件信箱' : '已取消對應', true)
      editing = null
      await load()
    } catch (e) { fail(e) } finally { busy = false }
  }

  async function toggle(r, enabled) {
    busy = true
    try {
      await api.toggleRecipient(r.id, enabled)
      notify(enabled ? '已恢復轉發' : '已停用轉發', true)
      await load()
    } catch (e) { fail(e) } finally { busy = false }
  }

  async function verify(r) {
    busy = true
    try {
      const res = await api.verifyRecipient(r.id)
      notify(
        res.sent
          ? `驗證信已寄到 ${r.address}，請對方點信裡的確認連結`
          : '已驗證過，或驗證信剛寄過（Cloudflare 有冷卻，請稍候再試）',
        res.sent
      )
      await load()
    } catch (e) { fail(e) } finally { busy = false }
  }

  async function add(mailbox) {
    const addr = draftAddr.trim().toLowerCase()
    if (!addr.includes('@')) return notify('請輸入完整的 Email 位址')
    busy = true
    try {
      const r = await api.addRecipient(mailbox, addr, draftLabel.trim())
      // 建不起來要講，否則會留下一筆「開著但一定退信」的收件人
      notify(r.warn ?? '已新增，驗證信已寄出（對方要點確認才收得到轉發）', !r.warn)
      adding = null; draftAddr = ''; draftLabel = ''
      await load()
    } catch (e) { fail(e) } finally { busy = false }
  }

  async function remove(r) {
    // 停用與移除是不同的事，講清楚再問
    if (!confirm(`移除 ${r.address}？\n\n只是暫時不想收的話請用停用 —— 移除之後要恢復得重打一次位址。`)) return
    try { await api.removeRecipient(r.id); notify('已移除', true); await load() } catch (e) { fail(e) }
  }

  async function purge(mailbox) {
    const n = rowsOf(mailbox).length
    if (
      !confirm(
        `永久刪除 ${mailbox}？\n\n底下 ${n} 筆收件人會一起消失，無法復原。\n\n` +
          '這個信箱沒有對應到任何平台，收不到任何信 —— 通常是設錯留下來的。'
      )
    )
      return
    try {
      const r = await api.purgeMailbox(mailbox)
      notify(`已刪除 ${mailbox} · ${r.removed} 筆`, true)
      await load()
    } catch (e) { fail(e) }
  }

  $effect(() => { load() })
</script>

<SubHeader title="轉發收件人" sub="平台寄來的驗證碼信會轉發給這些信箱" />

<div class="px-5 pt-3.5 flex flex-col gap-2.5">
  <div class="p-4 rounded-md bg-surface text-label leading-relaxed text-fg-muted text-pretty">
    每多一個人，驗證碼就多一份副本。面板穩定之後應該逐步關掉 ——
    <b class="font-semibold">關掉而不是移除</b>，之後要恢復不必重打位址。
    <br /><br />
    新增位址時面板會順手在 Cloudflare 建立它並寄出驗證信 ——
    <b class="font-semibold">對方要點過那封信才收得到轉發</b>。
  </div>

  {#each platforms as p (p.code)}
    {@const mailbox = boxes[p.code]}
    {@const rows = mailbox ? rowsOf(mailbox) : []}
    <div class="bg-surface rounded-lg overflow-hidden">
      <div class="px-4 py-3 bg-canvas">
        <div class="flex items-center justify-between gap-3">
          <span class="flex items-center gap-2 min-w-0">
            <PlatformMark code={p.code} name={p.name} color={p.color} size="sm" />
            <span class="text-item font-semibold truncate">{p.name}</span>
          </span>
          <button
            onclick={() => { editing = p.code; draftMailbox = mailbox ?? `@${defaultDomain}` }}
            class="min-h-0 text-meta font-medium text-ok shrink-0"
          >{mailbox ? '改信箱' : '設定信箱'}</button>
        </div>

        {#if editing === p.code}
          <div class="mt-2 flex gap-2">
            <input bind:value={draftMailbox} placeholder="disney@{defaultDomain}"
              autocapitalize="off" spellcheck="false"
              class="flex-1 min-w-0 px-3 py-2.5 rounded-sm border-[1.5px] border-line-firm
                     bg-surface font-mono text-body outline-none focus:border-fg" />
            <button onclick={() => saveMailbox(p.code)} disabled={busy}
              class="px-3.5 py-2.5 min-h-0 rounded-sm bg-fg text-canvas text-body font-medium
                     disabled:opacity-50">儲存</button>
            <button onclick={() => (editing = null)}
              class="px-2 py-2.5 min-h-0 text-body text-fg-muted">取消</button>
          </div>
          <p class="mt-1.5 text-meta leading-relaxed text-fg-faint text-pretty">
            這個平台的驗證碼信實際寄到哪個信箱。清空可以取消對應。
          </p>
        {:else if mailbox}
          <div class="mt-0.5 font-mono text-body font-medium truncate">{mailbox}</div>
        {:else}
          <div class="mt-1 text-label text-watch-fg leading-relaxed text-pretty">
            還沒設定收件信箱 —— 登記邀請時不會幫這個平台建立轉發。
          </div>
        {/if}
      </div>

      {#if mailbox}
        <div class="divide-y divide-line">
          {#each rows as r (r.id)}
            {@const st = status(r)}
            <div class="px-4 py-3">
              <div class="flex items-center justify-between gap-3">
                <div class="min-w-0">
                  <div class="font-mono text-body font-medium truncate {r.enabled ? '' : 'text-fg-faint'}">
                    {r.address}
                  </div>
                  <div class="mt-0.5 text-label truncate
                              {st.tone === 'bad' ? 'text-bad-fg' : 'text-fg-faint'}">
                    {r.label ? `${r.label} · ` : ''}{st.text}
                  </div>
                  {#if st.fix}
                    <!-- 兩種壞法同一支 API 都能修：建位址那支對未驗證的會重寄 -->
                    <button onclick={() => verify(r)} disabled={busy}
                      class="mt-1 min-h-0 text-meta font-medium text-ok disabled:opacity-50">
                      {r.cf_present === false ? '在 Cloudflare 建立並寄驗證信' : '重新發送驗證信'}
                    </button>
                  {/if}
                </div>
                <Toggle
                  checked={r.enabled}
                  disabled={busy}
                  label="轉發給 {r.address}"
                  onchange={(v) => toggle(r, v)}
                />
              </div>
              <button onclick={() => remove(r)}
                class="mt-1.5 min-h-0 text-meta font-medium text-bad-fg">移除</button>
            </div>
          {/each}
        </div>

        {#if adding === mailbox}
          <div class="p-3 bg-canvas flex flex-col gap-2">
            <input bind:value={draftAddr} placeholder="someone@example.com"
              autocapitalize="off" spellcheck="false"
              class="w-full px-3 py-2.5 rounded-sm border-[1.5px] border-line-firm bg-surface
                     font-mono text-body outline-none focus:border-fg" />
            <input bind:value={draftLabel} placeholder="備註（選填，例：二樓 阿明）"
              class="w-full px-3 py-2.5 rounded-sm border-[1.5px] border-line-firm bg-surface
                     text-body outline-none focus:border-fg" />
            <div class="flex gap-2">
              <button onclick={() => add(mailbox)} disabled={busy}
                class="flex-1 py-2.5 min-h-0 rounded-sm bg-fg text-canvas text-body font-medium disabled:opacity-50"
              >新增</button>
              <button onclick={() => (adding = null)}
                class="px-3 py-2.5 min-h-0 text-body text-fg-muted">取消</button>
            </div>
          </div>
        {:else}
          <button onclick={() => { adding = mailbox; draftAddr = ''; draftLabel = '' }}
            class="w-full py-3 min-h-0 border-t border-line text-label font-medium text-fg-muted"
          >＋ 新增收件人</button>
        {/if}
      {/if}
    </div>
  {/each}

  <!-- 不屬於任何平台的信箱。它們收不到任何信，卻看起來像有效的轉發目標 -->
  {#each orphans as mailbox (mailbox)}
    <div class="bg-surface rounded-lg overflow-hidden">
      <div class="px-4 py-3 bg-watch-bg">
        <span class="font-mono text-micro font-medium tracking-widest uppercase text-watch-fg">
          沒有對應到平台
        </span>
        <div class="mt-0.5 font-mono text-body font-medium truncate">{mailbox}</div>
        <p class="mt-1 text-label leading-relaxed text-fg-muted text-pretty">
          這個信箱不屬於任何平台，收不到任何信 —— 通常是設錯留下來的。
          要留著的話，到上面把它設成某個平台的收件信箱。
        </p>
      </div>
      <div class="divide-y divide-line">
        {#each rowsOf(mailbox) as r (r.id)}
          <div class="px-4 py-2.5 font-mono text-label text-fg-faint truncate">{r.address}</div>
        {/each}
      </div>
      <button onclick={() => purge(mailbox)}
        class="w-full py-3 min-h-0 border-t border-line text-label font-medium text-bad-fg"
      >永久刪除這個信箱</button>
    </div>
  {/each}

  {#if !cfEnabled}
    <p class="text-label leading-relaxed text-fg-faint text-pretty px-1">
      未設定 Cloudflare 帳戶或 token，因此查不到驗證狀態，也無法自動建立位址。
      需要帳戶層級的 <span class="font-mono">Email Routing Addresses</span> 讀寫權限。
    </p>
  {/if}
</div>
