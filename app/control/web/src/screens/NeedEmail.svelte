<script>
  import { notify, refresh, fail } from '../lib/state.svelte.js'
  import { api } from '../lib/api.js'

  // v6 遷移用。舊帳號的 email 是 NULL，而面板現在一律以信箱稱呼使用者、
  // 平台分權與轉發也都靠它。不補的話這個帳號會卡在半途。
  let email = $state('')
  let busy = $state(false)
  let err = $state(null)

  async function save() {
    busy = true
    err = null
    try {
      await api.setMyEmail(email.trim().toLowerCase())
      notify('已補上信箱', true)
      await refresh()
    } catch (e) {
      err = e.message
    } finally {
      busy = false
    }
  }
</script>

<div class="flex flex-col min-h-[100dvh] px-6 pt-16">
  <h1 class="text-head font-bold leading-tight tracking-tight">補上你的 Email</h1>
  <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
    面板改版後一律以信箱稱呼使用者，平台權限與驗證碼轉發也都靠它對應。
    你的帳號是改版前建立的，還沒有信箱。
  </p>
  <p class="mt-3 text-label leading-relaxed text-fg-faint text-pretty">
    只需要填一次。你既有的白名單、Passkey 與稽核紀錄都不受影響。
  </p>

  <input
    bind:value={email}
    type="email" inputmode="email" autocapitalize="off" spellcheck="false"
    placeholder="you@example.com"
    onkeydown={(e) => e.key === 'Enter' && save()}
    class="mt-6 w-full px-4 py-3.5 rounded-md bg-surface border-[1.5px] border-line-firm
           font-mono outline-none focus:border-fg"
  />

  {#if err}
    <div class="mt-4 p-4 rounded-md bg-bad-bg text-body text-bad-fg">{err}</div>
  {/if}

  <button
    onclick={save}
    disabled={busy || !email.includes('@')}
    class="mt-6 w-full py-5 rounded-lg bg-fg text-canvas text-lead font-semibold
           disabled:bg-line-firm disabled:text-fg-faint"
  >
    儲存
  </button>
</div>
