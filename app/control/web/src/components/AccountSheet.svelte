<script>
  import { app, notify, refresh } from '../lib/state.svelte.js'
  import { api, registerPasskey, passkeyError, guessDeviceName } from '../lib/api.js'
  import { since, stamp } from '../lib/time.js'
  import Sheet from './Sheet.svelte'
  import Toggle from './Toggle.svelte'
  import PushSheet from './PushSheet.svelte'
  import { pushState, disablePush } from '../lib/push.js'
  import IconKey from '~icons/lucide/key-round'
  import IconBell from '~icons/lucide/bell'
  import IconForward from '~icons/lucide/forward'
  import IconPlus from '~icons/lucide/plus'
  import IconLogout from '~icons/lucide/log-out'
  import IconAlert from '~icons/lucide/triangle-alert'

  // 設計稿的 16 個畫面裡沒有 passkey 管理與登出，但兩者都必須有地方去：
  // 只有一把 passkey 的人若弄丟裝置就再也登不進來，而登出是基本操作。
  // 收在首頁右上的信箱底下 —— member 看不到管理分頁，不能放那。
  // startAdding：首頁「建一把 Passkey」的卡片開這頁時，直接展開新增流程
  let { open = false, startAdding = false, onclose } = $props()

  let keys = $state([])
  let busy = $state(false)
  let adding = $state(false)
  let draft = $state('')
  let renaming = $state(null)

  const only = $derived(keys.length === 1)
  // 驗證碼登入的 session：這台裝置上沒有 Passkey，提示要講的是「這台沒有」，
  // 不是「帳號只剩一把」—— 兩件事的下一步不同。
  const otpSession = $derived(app.status?.auth_via === 'otp')

  // 兩層狀態：這台裝置有沒有訂閱（瀏覽器端），以及帳號層的兩顆開關
  // （跟著人跑）。分開是因為它們真的是兩件事。
  let push = $state('ready')
  let prefs = $state({ codes: true, expiry: false })
  let askPush = $state(false)
  // ⚠️ 撤銷 passkey 只擋登入，不會停掉推播 —— 弄丟的手機會繼續收到
  //    驗證碼。所以訂閱跟 passkey 一樣要列得出來、撤得掉。
  let subs = $state([])

  // ── 轉發 ──
  let fwd = $state(null)

  async function loadExtras() {
    push = await pushState()
    try { subs = await api.pushSubs() } catch {}
    try { prefs = await api.notifyPrefs() } catch {}
    try { fwd = await api.myForwarding() } catch (e) { notify(e.message) }
  }

  async function setPref(key, value) {
    const next = { ...prefs, [key]: value }
    prefs = next
    try { await api.setNotifyPrefs(next.codes, next.expiry) }
    catch (e) { notify(e.message); prefs = await api.notifyPrefs() }
  }

  async function turnPushOff() {
    try {
      await disablePush()
      push = await pushState()
      subs = await api.pushSubs()
      notify('已關閉這台裝置的通知', true)
    } catch (e) { notify(e.message) }
  }

  async function revokeSub(sub) {
    if (!confirm(`停止推播到「${sub.label || '未命名的裝置'}」？`)) return
    try {
      await api.pushUnsubscribe(sub.id)
      subs = await api.pushSubs()
      // 撤掉的可能就是眼前這台 —— 不重算的話上面還會寫著「已開啟」
      push = await pushState()
      notify('已停止推播到那台裝置', true)
    } catch (e) { notify(e.message) }
  }

  // 關掉之後只剩面板和通知看得到碼 —— 沒開通知的人就什麼提示都沒有了
  async function toggleForwarding(on) {
    if (!on && push !== 'on') {
      if (!confirm('關掉之後，新的驗證碼只會出現在面板裡 —— 你還沒開啟通知，不會收到任何提示。\n\n確定要關掉轉發嗎？')) return
    }
    try {
      await api.setMyForwarding(on)
      notify(on ? '已開啟轉發' : '已關閉轉發', true)
      fwd = await api.myForwarding()
    } catch (e) { notify(e.message) }
  }

  async function resendVerify() {
    try {
      const r = await api.resendForwardingVerify()
      notify(r.sent ? '驗證信已寄出，請到信箱點確認' : '已驗證過，或驗證信剛寄過（請稍候再試）', r.sent)
      fwd = await api.myForwarding()
    } catch (e) { notify(e.message) }
  }

  async function load() {
    try { keys = await api.passkeys() } catch (e) { notify(e.message) }
  }

  // 開啟時才載入。這些只有這裡在用，沒必要塞進 /api/status。
  $effect(() => {
    if (open) {
      load()
      loadExtras()
      if (startAdding) { adding = true; draft = '' }
    }
  })

  async function add() {
    busy = true
    try {
      await registerPasskey({ nickname: draft.trim() || guessDeviceName() })
      notify('已新增一把 Passkey', true)
      adding = false
      draft = ''
      await load()
      await refresh()
    } catch (e) {
      notify(passkeyError(e))
    } finally { busy = false }
  }

  async function rename(k) {
    try {
      await api.renamePasskey(k.id, draft.trim() || null)
      renaming = null
      await load()
    } catch (e) { notify(e.message) }
  }

  async function revoke(k) {
    const name = k.nickname || '這把 Passkey'
    if (!confirm(`撤銷「${name}」？那台裝置將無法再登入面板。`)) return
    try {
      await api.revokePasskey(k.id)
      notify('已撤銷', true)
      await load()
      await refresh()
    } catch (e) { notify(e.message) }
  }
</script>

<Sheet {open} {onclose} title="帳號">
  <div class="mt-4 p-4 rounded-md bg-canvas">
    <div class="font-mono text-body break-all">{app.status?.username}</div>
    <div class="mt-1 text-label text-fg-faint">
      {app.status?.is_admin ? '管理員' : '一般成員'}
    </div>
  </div>

  {#if otpSession}
    <div class="mt-3 flex gap-3 p-4 rounded-md bg-watch-bg">
      <IconAlert width="18" height="18" class="text-watch-fg shrink-0 mt-0.5" />
      <div>
        <div class="text-body font-semibold text-watch-fg">這台裝置還沒有 Passkey</div>
        <p class="mt-1 text-label leading-relaxed text-fg-strong text-pretty">
          你是用驗證碼登入的，這台裝置還沒有 Passkey。建一把之後登入就不必再收驗證碼；
          管理功能也只有 Passkey 登入的 session 能用。
        </p>
      </div>
    </div>
  {:else if only}
    <div class="mt-3 flex gap-3 p-4 rounded-md bg-watch-bg">
      <IconAlert width="18" height="18" class="text-watch-fg shrink-0 mt-0.5" />
      <div>
        <div class="text-body font-semibold text-watch-fg">只有一把 Passkey</div>
        <p class="mt-1 text-label leading-relaxed text-fg-strong text-pretty">
          這台裝置若遺失或重置，你將無法再登入面板，也就無法再授權任何網路 ——
          沒有密碼可以救。強烈建議現在就在另一台裝置上再註冊一把。
        </p>
      </div>
    </div>
  {/if}

  <!-- ── 通知 ── -->
  <div class="mt-5 flex items-center gap-2">
    <IconBell width="16" height="16" class="text-fg-faint" />
    <h3 class="text-body font-semibold">通知</h3>
  </div>

  {#if push === 'on'}
    <div class="mt-2 p-4 rounded-md bg-canvas flex flex-col gap-3">
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="text-item font-semibold">新驗證碼</div>
          <div class="mt-0.5 text-label text-fg-faint">平台寄來登入碼時通知我</div>
        </div>
        <Toggle checked={prefs.codes} label="新驗證碼通知"
                onchange={(v) => setPref('codes', v)} />
      </div>
      <div class="h-px bg-line"></div>
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="text-item font-semibold">授權快到期</div>
          <div class="mt-0.5 text-label text-fg-faint">白名單的 IP 剩 24 小時時提醒</div>
        </div>
        <Toggle checked={prefs.expiry} label="授權到期提醒"
                onchange={(v) => setPref('expiry', v)} />
      </div>
    </div>
    <!-- 推桿是帳號層的，這顆關的是這一台 -->
    <button onclick={turnPushOff}
      class="mt-2 w-full py-3 rounded-md border-[1.5px] border-line-firm
             text-item font-medium text-fg-muted"
    >關閉這台裝置的通知</button>
  {:else}
    <button onclick={() => (askPush = true)}
      class="mt-2 w-full py-3.5 rounded-md bg-ok text-white text-item font-semibold"
    >開啟通知</button>
    <p class="mt-1.5 px-1 text-label leading-relaxed text-fg-faint text-pretty">
      {#if push === 'homescreen'}
        iPhone 要先把面板加到主畫面才收得到通知，點上面看步驟。
      {:else if push === 'blocked'}
        這個瀏覽器擋掉了通知權限，要到網站設定裡改回來。
      {:else if push === 'unsupported'}
        這個瀏覽器不支援通知，驗證碼照樣會出現在面板上。
      {:else}
        新驗證碼一到就直接通知這台裝置，不用一直回來重新整理。
      {/if}
    </p>
  {/if}

  {#if subs.length}
    <!-- 唯一能停掉其他裝置推播的地方 -->
    <div class="mt-2 flex flex-col gap-1.5">
      <span class="px-1 text-label text-fg-faint">收得到通知的裝置 {subs.length} 台</span>
      {#each subs as sub (sub.id)}
        <div class="px-4 py-3 rounded-md bg-canvas flex items-center justify-between gap-3">
          <div class="min-w-0">
            <div class="text-item font-medium truncate">{sub.label || '未命名的裝置'}</div>
            <div class="mt-0.5 text-label text-fg-faint">
              {stamp(sub.created_at)} 開啟{#if sub.last_ok_at} · {since(sub.last_ok_at)}推送過{/if}
            </div>
          </div>
          <button onclick={() => revokeSub(sub)}
            class="min-h-0 text-meta font-medium text-bad-fg shrink-0">停止推播</button>
        </div>
      {/each}
      <p class="px-1 text-meta leading-relaxed text-fg-faint text-pretty">
        撤銷 Passkey 只擋登入，不會停掉推播。裝置遺失時記得也在這裡停掉它。
      </p>
    </div>
  {/if}

  <!-- ── 轉發到我的信箱 ── -->
  <div class="mt-5 flex items-center gap-2">
    <IconForward width="16" height="16" class="text-fg-faint" />
    <h3 class="text-body font-semibold">轉發到我的信箱</h3>
  </div>

  {#if fwd && fwd.address && fwd.registered}
    <div class="mt-2 p-4 rounded-md bg-canvas">
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <div class="font-mono text-item font-medium truncate">{fwd.address}</div>
          <div class="mt-0.5 text-label text-fg-faint">
            {fwd.mailboxes.length} 個平台信箱會轉發給你
          </div>
        </div>
        <Toggle checked={fwd.enabled} label="轉發到我的信箱"
                onchange={toggleForwarding} />
      </div>

      <!-- cf_checked_at 為 null 代表「還沒查過」，不是「尚未驗證」——
           混用會讓人以為自己沒點驗證信，實際上是面板沒設 token。 -->
      <!-- cf_checked_at 為 null = 還沒查過；cf_present 為 false = Cloudflare
           根本沒這個位址，驗證信從沒寄過 —— 叫他去信箱找會讓他白找。 -->
      {#if fwd.cf_enabled && fwd.cf_checked_at != null}
        <div class="mt-3 pt-3 border-t border-line">
          {#if fwd.cf_verified_at}
            <div class="text-label text-fg-faint">
              Cloudflare 已驗證・{since(fwd.cf_verified_at)}
            </div>
          {:else}
            <div class="text-label text-bad-fg leading-relaxed text-pretty">
              {#if fwd.cf_present === false}
                這個位址還沒登記到 Cloudflare，轉發一定會失敗 ——
                驗證信也還沒寄出去過。按下面那顆按鈕現在寄一封。
              {:else}
                這個位址還沒在 Cloudflare 完成驗證，轉發會失敗。
                請到信箱點驗證信裡的確認連結（也看一下垃圾郵件）。
              {/if}
            </div>
            <button onclick={resendVerify}
              class="mt-2 w-full py-2.5 min-h-0 rounded-sm border-[1.5px] border-line-firm
                     text-item font-medium"
            >{fwd.cf_present === false ? '寄出驗證信' : '重新發送驗證信'}</button>
          {/if}
        </div>
      {/if}
    </div>
  {:else if fwd && fwd.address}
    <p class="mt-2 p-4 rounded-md bg-canvas text-label leading-relaxed text-fg-muted text-pretty">
      管理員還沒把你的信箱設為轉發對象。驗證碼會出現在面板上，
      開啟通知後也會推到這台裝置。
    </p>
  {:else if fwd}
    <p class="mt-2 p-4 rounded-md bg-canvas text-label leading-relaxed text-fg-muted text-pretty">
      這個帳號還沒有 Email，無法設定轉發。
    </p>
  {/if}

  <div class="mt-5 flex items-center gap-2">
    <IconKey width="16" height="16" class="text-fg-faint" />
    <h3 class="text-body font-semibold">Passkey</h3>
    <span class="text-label text-fg-faint">{keys.length} 把</span>
  </div>

  <div class="mt-2 flex flex-col gap-2">
    {#each keys as k (k.id)}
      <div class="p-3.5 rounded-md bg-canvas">
        {#if renaming === k.id}
          <div class="flex gap-2">
            <input
              bind:value={draft}
              placeholder="例如：iPhone 15"
              onkeydown={(e) => e.key === 'Enter' && rename(k)}
              class="flex-1 min-w-0 px-3 py-2.5 rounded-sm border-[1.5px] border-line-firm
                     bg-surface text-body outline-none focus:border-fg"
            />
            <button onclick={() => rename(k)}
              class="px-3.5 py-2.5 min-h-0 rounded-sm bg-fg text-canvas text-body font-medium">儲存</button>
            <button onclick={() => (renaming = null)}
              class="px-2 py-2.5 min-h-0 text-body text-fg-muted">取消</button>
          </div>
        {:else}
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <div class="text-item font-medium truncate">{k.nickname || '未命名的裝置'}</div>
              <div class="mt-0.5 text-label text-fg-faint">
                {stamp(k.created_at)} 註冊 ·
                {#if k.last_used_at}
                  {since(k.last_used_at)}用過
                {:else}
                  <span class="text-watch-fg">從未用來登入</span>
                {/if}
              </div>
            </div>
          </div>
          <div class="mt-2.5 flex gap-2">
            <button
              onclick={() => { renaming = k.id; draft = k.nickname ?? '' }}
              class="flex-1 py-2.5 min-h-0 rounded-sm border-[1.5px] border-line-firm text-body font-medium"
            >重新命名</button>
            <!-- 最後一把不給撤銷 —— 沒有密碼可以救，刪光了就永遠登不進來。
                 後端也會擋，這裡只是不要讓人按了才看到錯誤。 -->
            <button
              onclick={() => revoke(k)}
              disabled={only}
              class="flex-1 py-2.5 min-h-0 rounded-sm border-[1.5px] border-bad/35 text-body
                     font-medium text-bad-fg disabled:opacity-35 disabled:border-line-firm
                     disabled:text-fg-faint"
            >撤銷</button>
          </div>
        {/if}
      </div>
    {/each}
  </div>

  {#if adding}
    <div class="mt-2 flex gap-2">
      <input
        bind:value={draft}
        placeholder={guessDeviceName()}
        onkeydown={(e) => e.key === 'Enter' && add()}
        class="flex-1 min-w-0 px-3 py-3 rounded-sm border-[1.5px] border-line-firm
               bg-canvas text-body outline-none focus:border-fg"
      />
      <button onclick={add} disabled={busy}
        class="px-4 py-3 min-h-0 rounded-sm bg-fg text-canvas text-body font-medium disabled:opacity-50"
      >{busy ? '…' : '註冊'}</button>
      <button onclick={() => (adding = false)}
        class="px-2 py-3 min-h-0 text-body text-fg-muted">取消</button>
    </div>
  {:else}
    <button
      onclick={() => { adding = true; draft = '' }}
      class="mt-2 w-full py-3.5 rounded-md border-[1.5px] border-dashed border-line-firm
             flex items-center justify-center gap-2 text-item font-medium text-fg-muted"
    >
      <IconPlus width="17" height="17" />
      在這台裝置新增一把
    </button>
  {/if}

  <button
    onclick={async () => { await api.logout(); location.reload() }}
    class="mt-4 w-full py-4 rounded-md flex items-center justify-center gap-2
           text-item font-medium text-bad-fg"
  >
    <IconLogout width="17" height="17" />
    登出
  </button>
</Sheet>

<PushSheet open={askPush} onclose={async () => { askPush = false; push = await pushState() }} />
