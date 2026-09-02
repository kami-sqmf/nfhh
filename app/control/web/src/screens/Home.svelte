<script>
  import { app, go, fail, notify, refresh } from '../lib/state.svelte.js'
  import { api } from '../lib/api.js'
  import { mailList } from '../lib/mail.svelte.js'
  import CodeCard from '../components/CodeCard.svelte'
  import AccountSheet from '../components/AccountSheet.svelte'
  import AllowSheet from '../components/AllowSheet.svelte'
  import MailView from '../components/MailView.svelte'
  import PushSheet from '../components/PushSheet.svelte'
  import { pushState } from '../lib/push.js'
  import IconSettings from '~icons/lucide/settings'

  // 1a：首頁以最新驗證碼為主，授權入口收在「遇到同戶裝置問題？」區塊裡。
  // 家人多數時候只是要一組碼，不該先被防火牆概念擋住。
  //
  // 清單、輪詢與「點開才取全文」跟驗證碼分頁、收件匣一字不差，
  // 共用 lib/mail.svelte.js 那一份。
  const list = mailList(api.mails)
  const mails = $derived(list.mails)
  let account = $state(false)
  let authorize = $state(false)
  let askPush = $state(false)
  let fwd = $state(null)

  // 這台裝置第一次進面板就問。旗標存 localStorage 而非 DB ——
  // 訂閱本來就是每台裝置一筆，換手機時應該要再問一次。
  const ASKED = 'nfhh:push-asked'

  async function maybeAskPush() {
    try {
      if (localStorage.getItem(ASKED)) return
    } catch {
      return // 隱私模式讀不到 storage，問了也記不住
    }
    // 已經開了、或根本推不動，就不要拿彈層擋人
    const s = await pushState()
    if (s === 'on' || s === 'unsupported' || s === 'blocked') return
    try { localStorage.setItem(ASKED, '1') } catch {}
    askPush = true
  }

  // ⚠️ 轉發壞掉完全沒有徵兆：面板上開著、Worker 每次退信、信退在
  //    Cloudflare 那邊。所以擺首頁，不是收在設定裡等人自己發現。
  const fwdBroken = $derived(
    fwd?.registered &&
    fwd.enabled &&
    fwd.cf_enabled &&
    fwd.cf_checked_at != null &&
    !fwd.cf_verified_at
  )

  async function resendVerify() {
    try {
      const r = await api.resendForwardingVerify()
      notify(
        r.sent ? '驗證信已寄出，請到信箱點確認連結' : '驗證信剛寄過，請稍候再試',
        r.sent
      )
      fwd = await api.myForwarding()
    } catch (e) { fail(e) }
  }

  const platform = (code) => app.status?.platforms?.find((p) => p.code === code)
  const platformName = (code) => platform(code)?.name ?? code

  const latest = $derived(mails[0] ?? null)
  const rest = $derived(Math.max(0, mails.length - 1))

  $effect(() => { list.load() })
  $effect(() => {
    maybeAskPush()
    api.myForwarding().then((r) => (fwd = r)).catch(() => {})
  })

  // 驗證碼時效很短，自動輪詢；切到背景就停，不浪費家人的電池與流量。
  $effect(() => {
    const t = setInterval(() => { if (!document.hidden) list.load() }, 20000)
    return () => clearInterval(t)
  })
</script>

<header class="flex items-center justify-between gap-3 px-5 py-2 bg-surface">
  <span class="font-mono text-body font-medium">OTT 共享控制台</span>
  <!-- 信箱本身早就能開個人設定，但一段純文字看起來不像按鈕。
       把信箱與齒輪包成同一顆膠囊：外觀說明「這裡可以按」，
       點文字或點圖標都是同一個動作。齒輪 settings 與管理分頁的
       settings-2（推桿）刻意不同 —— 這裡是個人設定，不是站台管理。 -->
  <button
    onclick={() => (account = true)}
    aria-label="開啟個人設定"
    class="flex items-center gap-1.5 max-w-[60%] pl-3.5 pr-3 rounded-pill bg-canvas"
  >
    <span class="font-mono text-body text-fg-muted truncate">{app.status?.username}</span>
    <IconSettings width="15" height="15" class="text-fg-faint shrink-0" />
  </button>
</header>

<div class="px-5 pt-4 flex flex-col gap-3 min-h-[calc(100dvh-141px)]">
  {#if fwdBroken}
    <!-- 只在「開著但一定送不到」時出現 —— 常駐的警告會變成背景雜訊 -->
    <div class="rounded-md bg-watch-bg p-4">
      <div class="text-item font-semibold text-watch-fg">轉發到你的信箱收不到</div>
      <p class="mt-1 text-label leading-relaxed text-fg-muted text-pretty">
        {#if fwd.cf_present === false}
          <span class="font-mono">{fwd.address}</span>
          還沒登記到 Cloudflare，驗證碼轉發過去會直接退信 —— 驗證信也還沒寄出去過。
        {:else}
          <span class="font-mono">{fwd.address}</span>
          還沒完成 Cloudflare 驗證，驗證碼轉發過去會直接退信。
          請到信箱點驗證信裡的確認連結（也看一下垃圾郵件）。
        {/if}
      </p>
      <button
        onclick={resendVerify}
        class="mt-2.5 w-full py-3 rounded-sm border-[1.5px] border-watch/40 text-item
               font-semibold text-watch-fg"
      >{fwd.cf_present === false ? '寄出驗證信' : '重新發送驗證信'}</button>
    </div>
  {/if}

  {#if latest}
    <CodeCard mail={latest} platformName={platformName(latest.platform)} platformColor={platform(latest.platform)?.color}
              showMailbox onview={list.view} />
    <div class="flex items-baseline justify-between px-1">
      <span class="text-body text-fg-faint">
        {rest > 0 ? `14 天內還有 ${rest} 組` : '14 天內沒有其他驗證碼'}
      </span>
      <button onclick={() => go('codes')} class="min-h-0 text-body text-ok">看全部驗證碼</button>
    </div>
  {:else}
    <div class="bg-surface rounded-lg p-5 text-body leading-relaxed text-fg-muted text-pretty">
      {#if !app.status?.my_platforms?.length}
        你還沒有被授權任何平台。請管理員到「成員管理」把平台開給你。
      {:else}
        目前沒有驗證碼。平台寄出驗證信後會自動出現在這裡。
      {/if}
    </div>
  {/if}

  <div class="flex-1 min-h-4"></div>

  <!-- 授權入口。紅底不是警告，是「這裡有個問題可以解」的入口。 -->
  <div class="rounded-md bg-bad-bg p-4">
    <div class="text-item font-semibold text-bad-fg">遇到同戶裝置問題？</div>
    <p class="mt-1 text-label leading-relaxed text-fg-muted text-pretty">
      目前只建議電視出現同戶裝置限制再套用。
    </p>

    <div class="mt-2.5 flex items-center justify-between gap-3 px-3.5 py-2.5 rounded-sm bg-surface/75">
      <span class="flex items-baseline gap-2 min-w-0">
        <span class="font-mono text-micro font-medium tracking-widest uppercase text-fg-faint shrink-0">出口 IP</span>
        <span class="font-mono text-lead font-medium truncate">{app.status?.my_ip ?? '取不到'}</span>
      </span>
      <span class="font-mono text-micro font-medium shrink-0
                   {app.status?.my_ip_allowed ? 'text-ok-fg' : 'text-bad-fg'}">
        {app.status?.my_ip_allowed ? '已授權' : '尚未授權'}
      </span>
    </div>

    <button
      onclick={() => (authorize = true)}
      class="mt-2.5 w-full py-3 rounded-sm border-[1.5px] border-bad/40 text-item font-semibold text-bad-fg"
    >
      {app.status?.my_ip_allowed ? '延長授權期限' : '授權這個網路'}
    </button>
  </div>
</div>

<AccountSheet open={account} onclose={() => (account = false)} />
<PushSheet open={askPush} onclose={() => (askPush = false)} />
<MailView mail={list.viewing} onclose={() => (list.viewing = null)} />
<AllowSheet open={authorize} onclose={() => (authorize = false)} ondone={refresh} />
