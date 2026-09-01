<script>
  import { notify, refresh } from '../lib/state.svelte.js'
  import { registerPasskey, passkeyError } from '../lib/api.js'

  // 設計稿沒有這一頁 —— 它假設系統已經在跑了。但第一個帳號總得有人建，
  // 而那時還沒有 admin 能登記信箱，所以走一次性碼（印在容器日誌）。
  //
  // 這條路徑刻意不要求 Email 驗證碼：面板還沒跑過時寄信服務未必設好，
  // 要求信件送達才能建第一個帳號是個死結。
  let email = $state('')
  let token = $state('')
  let busy = $state(false)
  let err = $state(null)

  async function create() {
    busy = true
    err = null
    try {
      await registerPasskey({ email: email.trim().toLowerCase(), bootstrapToken: token.trim() })
      notify('帳號已建立', true)
      await refresh()
    } catch (e) {
      err = passkeyError(e)
    } finally {
      busy = false
    }
  }
</script>

<div class="flex flex-col min-h-[100dvh] px-6 pt-16">
  <h1 class="text-hero font-bold leading-tight tracking-tight">建立第一個帳號</h1>
  <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
    這個帳號會是管理員。一次性註冊碼印在容器日誌裡，用
    <span class="font-mono">docker logs nfhh-control</span> 查看。
  </p>

  <label class="block mt-6">
    <span class="text-label text-fg-muted">你的 Email</span>
    <input
      bind:value={email}
      type="email" inputmode="email" autocapitalize="off" spellcheck="false"
      class="mt-1.5 w-full px-4 py-3.5 rounded-md bg-surface border-[1.5px] border-line-firm
             font-mono outline-none focus:border-fg"
    />
  </label>

  <label class="block mt-4">
    <span class="text-label text-fg-muted">一次性註冊碼</span>
    <input
      bind:value={token}
      autocapitalize="off" spellcheck="false"
      class="mt-1.5 w-full px-4 py-3.5 rounded-md bg-surface border-[1.5px] border-line-firm
             font-mono text-body outline-none focus:border-fg"
    />
  </label>

  {#if err}
    <div class="mt-4 p-4 rounded-md bg-bad-bg text-body text-bad-fg">{err}</div>
  {/if}

  <button
    onclick={create}
    disabled={busy || !email.trim() || !token.trim()}
    class="mt-6 w-full py-5 rounded-lg bg-fg text-canvas text-lead font-semibold
           disabled:bg-line-firm disabled:text-fg-faint"
  >
    註冊 Passkey
  </button>
</div>
