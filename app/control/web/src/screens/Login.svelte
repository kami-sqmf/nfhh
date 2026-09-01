<script>
  import { app, notify, refresh } from '../lib/state.svelte.js'
  import { loginPasskey, loginDiscoverable, supportsConditionalUi, passkeyError } from '../lib/api.js'

  // 設計 1j：沒有輸入欄位，一顆按鈕就登入。走可探索憑證 ——
  // 裝置自己知道有哪些帳號，不必先告訴伺服器我是誰。
  //
  // 但 webauthn-rs 0.5 註冊時送的是 residentKey: "discouraged"，
  // 所以不是每一把 passkey 都可探索（見 main.rs 的登入段註解）。
  // Email 登入因此保留成退路，預設收起來不佔版面。
  let busy = $state(false)
  let fallback = $state(false)
  let email = $state(localStorage.getItem('nfhh:last-email') ?? '')

  async function done(username) {
    if (username) localStorage.setItem('nfhh:last-email', username)
    notify('登入成功', true)
    await refresh()
  }

  async function passkey() {
    busy = true
    try {
      const r = await loginDiscoverable()
      await done(r.username)
    } catch (e) {
      // 不管哪種失敗都指路到退路 —— 使用者要的是「怎麼進去」，
      // 不是知道 WebAuthn 規格的哪一條沒滿足。
      notify(passkeyError(e))
      fallback = true
    } finally {
      busy = false
    }
  }

  async function byEmail() {
    if (!email.trim()) return notify('請輸入 Email')
    busy = true
    try {
      await loginPasskey(email.trim())
      await done(email.trim())
    } catch (e) {
      notify(passkeyError(e))
    } finally {
      busy = false
    }
  }

  // Conditional UI：不跳窗，把 passkey 掛進輸入框的自動填入。
  // 只在展開退路、而且瀏覽器支援時啟動；元件卸載要 abort，
  // 否則這個等待中的請求會擋掉之後任何一次 credentials.get()。
  $effect(() => {
    if (!fallback) return
    const ctrl = new AbortController()
    ;(async () => {
      if (!(await supportsConditionalUi())) return
      try {
        const r = await loginDiscoverable({ conditional: true, signal: ctrl.signal })
        await done(r.username)
      } catch {
        // 使用者沒選、或按了主按鈕把它取消掉 —— 都不是錯誤
      }
    })()
    return () => ctrl.abort()
  })
</script>

<div class="flex flex-col min-h-[100dvh] px-6">
  <div class="flex-1 flex flex-col items-center justify-end text-center pb-8">
    <!-- 面板網域不寫死：顯示的就是使用者連進來的那個，跟 WebAuthn 的 RP ID 同源 -->
    <div class="font-mono text-label font-medium tracking-[0.14em] uppercase text-fg-faint">
      {location.hostname}
    </div>
    <h1 class="mt-2 text-hero font-bold leading-tight tracking-tight">OTT 共享控制台</h1>
    <p class="mt-2 text-item leading-relaxed text-fg-muted">
      Netflix / Disney+ 驗證碼 / 同戶裝置問題
    </p>
  </div>

  <button
    onclick={passkey}
    disabled={busy}
    class="w-full py-5 rounded-lg bg-fg text-canvas text-lead font-semibold
           flex items-center justify-center gap-3 disabled:opacity-50"
  >
    <span class="w-6 h-6 rounded-chip bg-canvas/20 grid place-items-center font-mono text-body font-semibold">P</span>
    使用 Passkey 登入
  </button>

  {#if fallback}
    <div class="mt-3">
      <input
        bind:value={email}
        type="email"
        inputmode="email"
        autocomplete="username webauthn"
        autocapitalize="off"
        spellcheck="false"
        placeholder="Email 或帳號"
        onkeydown={(e) => e.key === 'Enter' && byEmail()}
        class="w-full px-4 py-3.5 rounded-md bg-surface border-[1.5px] border-line-firm
               font-mono outline-none focus:border-fg"
      />
      <button
        onclick={byEmail}
        disabled={busy}
        class="mt-2 w-full py-3.5 rounded-md border-[1.5px] border-line-firm text-item font-medium disabled:opacity-50"
      >登入</button>
      <!-- v6 之前的帳號還沒有信箱，登入要用原本的帳號名。
           後端會先查 email、查不到再退回 username。 -->
      <p class="mt-2 text-label leading-relaxed text-fg-faint text-pretty">
        改版前建立的帳號還沒有信箱，請填原本的帳號名稱；登入後面板會請你補上 Email。
      </p>
    </div>
  {:else}
    <button
      onclick={() => (fallback = true)}
      class="mt-2.5 w-full py-2.5 min-h-0 text-body text-fg-faint"
    >登不進去？改用 Email 登入</button>
  {/if}

  <div class="flex-1 min-h-[60px]"></div>

  <div class="flex items-center gap-3 mb-4">
    <div class="flex-1 h-px bg-line-firm"></div>
    <span class="text-label text-fg-faint">第一次加入</span>
    <div class="flex-1 h-px bg-line-firm"></div>
  </div>

  <button
    onclick={() => (app.authStep = 'join')}
    disabled={!app.status?.join_enabled}
    class="w-full py-4 rounded-md border-[1.5px] border-line-firm text-lead font-semibold
           disabled:opacity-40"
  >
    用 Email 加入
  </button>
  {#if !app.status?.join_enabled}
    <p class="mt-2 text-center text-label text-fg-faint">尚未設定寄信服務，請聯絡管理員</p>
  {/if}

  <div class="h-8"></div>
</div>
