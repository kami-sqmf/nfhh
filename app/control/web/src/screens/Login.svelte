<script>
  import { app, notify, refresh } from '../lib/state.svelte.js'
  import { loginDiscoverable, passkeyError } from '../lib/api.js'

  // 設計 1j：沒有輸入欄位，一顆按鈕就登入。走可探索憑證 ——
  // 裝置自己知道有哪些帳號，不必先告訴伺服器我是誰。
  //
  // 退路是 Email 驗證碼（不靠 Passkey），給換了手機、或憑證弄丟的人。
  // 它跟「用 Email 加入」走同一個寄信服務，所以 join_enabled 關著時一起停用。
  let busy = $state(false)

  async function passkey() {
    busy = true
    try {
      await loginDiscoverable()
      notify('登入成功', true)
      await refresh()
    } catch (e) {
      // 訊息本身已指路到退路（見 passkeyError）—— 使用者要的是「怎麼進去」，
      // 不是知道 WebAuthn 規格的哪一條沒滿足。
      notify(passkeyError(e))
    } finally {
      busy = false
    }
  }
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

  <button
    onclick={() => (app.authStep = 'loginemail')}
    disabled={!app.status?.join_enabled}
    class="mt-2.5 w-full py-2.5 min-h-0 text-body text-fg-faint disabled:opacity-40"
  >登不進去？改用 Email 驗證碼登入</button>

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
    <!-- 驗證碼登入與加入都靠寄信；沒設寄信服務時兩條路一起關，說明放一次就好 -->
    <p class="mt-2 text-center text-label text-fg-faint">尚未設定寄信服務，請聯絡管理員</p>
  {/if}

  <div class="h-8"></div>
</div>
