<script>
  import { app, notify, fail, refresh } from '../../lib/state.svelte.js'
  import { api } from '../../lib/api.js'
  import { left } from '../../lib/time.js'
  import SubHeader from '../../components/SubHeader.svelte'
  import PlatformMark from '../../components/PlatformMark.svelte'
  import Copyable from '../../components/Copyable.svelte'

  // 設計 1l ＋ 平台分權（設計稿沒有這塊，是你補的需求）。
  let members = $state([])
  let invites = $state([])
  let expanded = $state(null)
  let adding = $state(false)
  let draft = $state('')
  // 登記當下就選好要給哪些平台。v7 之前是「登記 → 對方註冊 → admin 再回來
  // 開平台」，中間那段空窗讓家人註冊完看到的是空的驗證碼分頁。
  let draftPlatforms = $state([])

  const platforms = $derived(app.status?.platforms ?? [])
  const admins = $derived(members.filter((m) => m.role === 'admin'))
  const users = $derived(members.filter((m) => m.role !== 'admin'))

  // 兩道在畫面上就先擋掉的護欄（後端也各擋一次）：
  //   自己 —— 刪了會把自己登出，而且多半是誤按
  //   最後一個 admin —— 面板會永久失去管理能力，沒有介面能救
  const isMe = (m) => m.label === app.status?.username
  const lastAdmin = (m) => m.role === 'admin' && admins.length <= 1
  const canRemove = (m) => !isMe(m) && !lastAdmin(m)

  async function load() {
    try {
      ;[members, invites] = await Promise.all([api.members(), api.invites()])
    } catch (e) { fail(e) }
  }

  async function setRole(m, role) {
    if (m.role === role) return
    try {
      await api.setRole(m.id, role)
      notify(`${m.label} 已改為 ${role === 'admin' ? 'Admin' : 'User'}`, true)
      await load()
      await refresh()
    } catch (e) { fail(e) }
  }

  async function togglePlatform(m, code) {
    try {
      if (m.platforms.includes(code)) await api.revoke(m.id, code)
      else await api.grant(m.id, code)
      await load()
      await refresh()
    } catch (e) { fail(e) }
  }

  // 登記完成的那一刻拿到的邀請連結。**只有這一刻拿得到** —— 後端只存雜湊，
  // 想再看一次只能重新登記換一條。所以就算信寄出去了也留在畫面上，
  // admin 可能想改用 LINE 之類的管道再傳一次。
  let issued = $state(null)

  async function invite() {
    const v = draft.trim().toLowerCase()
    if (!v.includes('@')) return notify('請輸入完整的 Email 位址')
    try {
      const r = await api.invite(v, draftPlatforms)
      // 後端回應不含平台，但提示要講「加進了幾個平台的轉發」，帶著送過去的那份
      issued = { ...r, platforms: draftPlatforms }
      // 寄信失敗不是登記失敗：位址已經可以用了，連結也還在下面。
      if (r.sent) notify(`已登記並寄出邀請函給 ${r.email}`, true)
      else notify(`已登記 ${r.email}，但${r.warn}`)
      draft = ''
      draftPlatforms = []
      adding = false
      await load()
    } catch (e) { fail(e) }
  }

  const toggleDraft = (code) =>
    (draftPlatforms = draftPlatforms.includes(code)
      ? draftPlatforms.filter((c) => c !== code)
      : [...draftPlatforms, code])

  // 編輯既有的登記 = 用同一個位址重新登記，平台會被覆寫
  function edit(i) {
    adding = true
    issued = null
    draft = i.email
    draftPlatforms = [...i.platforms]
  }

  async function uninvite(email) {
    try { await api.uninvite(email); await load() } catch (e) { fail(e) }
  }

  async function remove(m) {
    // 把後果講在前面。「移除成員」聽起來像停用，實際上是把那個人所有的
    // 存取與收件能力一次收掉，而且不可復原。
    const n = m.entries.length
    const warn = n > 0 ? `\n\n・他授權的 ${n} 個網路會一併移除，那些網路上的裝置會立刻失去存取` : ''
    if (
      !confirm(
        `移除 ${m.label}？${warn}` +
          '\n・他的 Passkey、平台授權、通知訂閱都會消失' +
          '\n・轉發到他信箱的登記會一併移除，他不會再收到驗證碼' +
          '\n\n無法復原。' +
          '\n\n注意：Cloudflare 上那個轉發位址不會被刪（那是帳戶層級的共用資源），' +
          '而 Worker 的 FORWARD_MAP 要自己去拿掉 —— 面板停機時它才是生效的那份名單。'
      )
    )
      return
    try {
      const r = await api.removeMember(m.id)
      const bits = [
        r.removed_entries > 0 ? `白名單 ${r.removed_entries} 筆` : null,
        r.removed_recipients > 0 ? `轉發 ${r.removed_recipients} 筆` : null,
      ].filter(Boolean)
      notify(
        bits.length ? `已移除 ${m.label} · 一併移除 ${bits.join('、')}` : `已移除 ${m.label}`,
        true
      )
      await load()
      await refresh()
    } catch (e) { fail(e) }
  }

  $effect(() => { load() })
</script>

<SubHeader title="成員管理" sub="{members.length} 人 · Admin {admins.length} · User {users.length}" />

<div class="px-5 pt-3 flex flex-col gap-2">
  {#each [['Admin', admins, '可管理全部設定'], ['User', users, '只能管自己的 IP']] as [group, list, note] (group)}
    <div class="flex items-baseline justify-between px-1 pt-1.5">
      <span class="font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
        {group} · {list.length} 人
      </span>
      <span class="text-meta text-fg-faint">{note}</span>
    </div>

    {#each list as m (m.id)}
      <div class="bg-surface rounded-md px-4 py-3">
        <div class="flex items-center justify-between gap-3">
          <span class="flex items-center gap-2 min-w-0">
            <span class="font-mono text-body font-semibold truncate">{m.label}</span>
            {#if m.label === app.status?.username}
              <span class="text-meta text-fg-faint shrink-0">你自己</span>
            {/if}
          </span>
          <div class="flex gap-0.5 p-0.5 rounded-pill bg-wash shrink-0">
            {#each [['admin', 'Admin'], ['member', 'User']] as [id, label] (id)}
              <button
                onclick={() => setRole(m, id)}
                class="px-2.5 py-1 min-h-0 rounded-pill text-micro
                       {m.role === id ? 'bg-fg text-canvas font-semibold' : 'text-fg-faint font-medium'}"
              >{label}</button>
            {/each}
          </div>
        </div>

        <!-- 平台分權。設計稿沒有這塊 —— 一個人可能只該拿到 Netflix 的碼。 -->
        <div class="mt-2.5 pt-2 border-t border-line">
          <div class="text-meta text-fg-faint">可看到哪些平台的驗證碼</div>
          <div class="mt-1.5 flex flex-wrap gap-1.5">
            {#each platforms as p (p.code)}
              <button
                onclick={() => togglePlatform(m, p.code)}
                class="px-3 py-1.5 min-h-0 rounded-pill text-label font-medium
                       {m.platforms.includes(p.code)
                         ? 'bg-ok-bg text-ok-fg'
                         : 'border-[1.5px] border-dashed border-line-firm text-fg-faint'}"
              >{p.name}</button>
            {:else}
              <span class="text-label text-fg-faint">沒有啟用中的平台</span>
            {/each}
          </div>
        </div>

        <div class="mt-2 pt-2 border-t border-line flex items-center justify-between gap-3">
          <button
            onclick={() => (expanded = expanded === m.id ? null : m.id)}
            class="flex items-center gap-2 text-meta text-fg-muted min-h-0 min-w-0"
          >
            <span class="truncate">新增了 {m.entries.length} 個 IP · Passkey {m.passkey_count} 把</span>
            <span class="font-medium text-ok shrink-0">{expanded === m.id ? '收合' : '展開'}</span>
          </button>
          {#if canRemove(m)}
            <button onclick={() => remove(m)}
              class="min-h-0 text-meta font-medium text-bad-fg shrink-0">移除成員</button>
          {:else}
            <span class="text-meta text-fg-faint shrink-0">
              {isMe(m) ? '不能移除自己' : '最後一個管理員'}
            </span>
          {/if}
        </div>

        {#if expanded === m.id}
          <div class="mt-1.5 flex flex-col gap-1.5">
            {#each m.entries as e (e.ip)}
              <div class="flex items-center justify-between gap-3 font-mono text-label">
                <span class="truncate">
                  {e.ip} <span class="text-fg-faint">{e.label ?? '未命名'} · 剩 {left(e.expires_at)}</span>
                </span>
              </div>
            {:else}
              <span class="text-label text-fg-faint">尚未新增任何 IP</span>
            {/each}
            <!-- 這裡刻意不提供「查看查詢明細」：那份只有條目擁有者拿得到。 -->
          </div>
        {/if}
      </div>
    {/each}
  {/each}

  <div class="bg-surface rounded-md p-4 mt-1.5">
    <div class="flex items-baseline justify-between">
      <h2 class="text-body font-semibold">等待註冊的邀請</h2>
      <span class="text-meta text-fg-faint">不過期 · admin 可撤銷</span>
    </div>

    <div class="mt-2 divide-y divide-line">
      {#each invites as i (i.email)}
        <div class="py-2.5 flex items-center justify-between gap-3">
          <div class="min-w-0">
            <!-- 已撤銷與已註冊的都不會出現在這裡（後端就濾掉了）——
                 這份清單回答的是「還有誰沒進來」。註冊完的人在上面的成員
                 清單裡，那邊連平台授權都顯示得更準確。歷史看稽核。 -->
            <div class="font-mono text-label truncate">{i.email}</div>
            <div class="mt-0.5 text-meta text-fg-faint truncate">
              {i.invited_by ?? '未知'} 登記 · 尚未註冊
            </div>
            <!-- 註冊完成時會自動授予的平台。空的話講明白，否則 admin 會以為
                 對方註冊完就能看到驗證碼。 -->
            <div class="mt-1 flex flex-wrap items-center gap-1.5">
              {#each i.platforms as c (c)}
                {@const p = platforms.find((x) => x.code === c)}
                <span class="flex items-center gap-1 px-2 py-0.5 rounded-pill bg-ok-bg text-ok-fg text-meta">
                  <PlatformMark code={c} name={p?.name ?? c} color={p?.color} size="sm" />
                  {p?.name ?? c}
                </span>
              {:else}
                <span class="text-meta text-watch-fg">未指定平台 · 註冊後看不到驗證碼</span>
              {/each}
            </div>
          </div>
          <div class="flex flex-col gap-1 shrink-0">
            <button onclick={() => edit(i)} class="min-h-0 text-meta font-medium text-ok">修改</button>
            <button onclick={() => uninvite(i.email)} class="min-h-0 text-meta font-medium text-bad-fg">
              撤銷
            </button>
          </div>
        </div>
      {:else}
        <p class="py-2.5 text-label text-fg-faint">沒有等待註冊的邀請。</p>
      {/each}
    </div>

    <p class="mt-2 text-meta leading-relaxed text-fg-faint text-pretty">
      登記後會順帶寄一封邀請函過去，信裡的連結按下去就能直接建 Passkey。
      收不到信也沒關係：對方到首頁選「用 Email 加入」，輸入完全相同的位址、
      填入寄到該信箱的驗證碼一樣能註冊。
    </p>

    {#if issued}
      <div class="mt-2.5 p-3 rounded-sm bg-canvas">
        <div class="flex items-baseline justify-between gap-3">
          <span class="font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
            {issued.email} 的邀請連結
          </span>
          <button onclick={() => (issued = null)} class="min-h-0 text-meta text-fg-faint">收起</button>
        </div>
        <p class="mt-1.5 text-meta leading-relaxed text-fg-faint text-pretty">
          {issued.sent ? '已寄到對方信箱。' : '邀請函沒寄出去。'}
          這條連結只在現在看得到，離開這頁就找不回來了 —— 要再發一條請重新登記。
          它等同那個信箱本人，別貼在公開的地方。
        </p>
        <div class="mt-2">
          <Copyable value={issued.link} />
        </div>

        {#if issued.platforms?.length}
          <!-- 自動新增的轉發不會出現在 Worker 的 FORWARD_MAP 裡，而那是面板
               停機時唯一的退路。平常讀不到、漂掉也沒徵兆，直到停機那天。 -->
          <p class="mt-2.5 p-2.5 rounded-sm bg-watch-bg text-meta leading-relaxed
                    text-watch-fg text-pretty">
            已一併把 {issued.email} 加進 {issued.platforms.length} 個平台的轉發名單，
            並在 Cloudflare 建立位址（對方要點驗證信才會生效）。
            <br />
            記得把它補進 Worker 的 <span class="font-mono">FORWARD_MAP</span> ——
            那是面板停機時唯一的退路，這裡的自動新增碰不到它。
          </p>
        {/if}
      </div>
    {/if}

    {#if adding}
      <div class="mt-2.5 p-3 rounded-sm bg-canvas">
        <input
          bind:value={draft}
          placeholder="someone@example.com"
          autocapitalize="off" spellcheck="false"
          onkeydown={(e) => e.key === 'Enter' && invite()}
          class="w-full px-3 py-2.5 rounded-sm border-[1.5px] border-line-firm
                 bg-surface font-mono text-body outline-none focus:border-fg"
        />

        <div class="mt-2.5 text-meta text-fg-faint">註冊完成時自動授予</div>
        <div class="mt-1.5 flex flex-wrap gap-1.5">
          {#each platforms as p (p.code)}
            <button
              onclick={() => toggleDraft(p.code)}
              class="flex items-center gap-1.5 px-3 py-1.5 min-h-0 rounded-pill text-label font-medium
                     {draftPlatforms.includes(p.code)
                       ? 'bg-ok-bg text-ok-fg'
                       : 'border-[1.5px] border-dashed border-line-firm text-fg-faint'}"
            >
              <PlatformMark code={p.code} name={p.name} color={p.color} size="sm" />
              {p.name}
            </button>
          {:else}
            <span class="text-label text-fg-faint">沒有啟用中的平台</span>
          {/each}
        </div>
        {#if !draftPlatforms.length}
          <p class="mt-2 text-meta leading-relaxed text-watch-fg text-pretty">
            沒選平台的話，對方註冊完會看不到任何驗證碼 —— 你得再回這頁補。
          </p>
        {/if}

        <div class="mt-3 flex gap-2">
          <button onclick={invite}
            class="flex-1 py-2.5 min-h-0 rounded-sm bg-fg text-canvas text-body font-medium">登記</button>
          <button onclick={() => { adding = false; draft = ''; draftPlatforms = [] }}
            class="px-3 py-2.5 min-h-0 text-body text-fg-muted">取消</button>
        </div>
      </div>
    {:else}
      <button
        onclick={() => { adding = true; draft = ''; draftPlatforms = [] }}
        class="mt-2.5 w-full py-2.5 min-h-0 rounded-sm border-[1.5px] border-dashed border-line-firm
               text-label font-medium text-fg-muted"
      >＋ 登記邀請 Email</button>
    {/if}
  </div>
</div>
