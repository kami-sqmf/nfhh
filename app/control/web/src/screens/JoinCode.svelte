<script>
  import { app, notify, refresh } from '../lib/state.svelte.js'
  import { api, registerPasskey, passkeyError } from '../lib/api.js'
  import OtpInput from '../components/OtpInput.svelte'

  let code = $state('')
  let busy = $state(false)
  let err = $state(null)
  let verified = $state(false)
  let cooldown = $state(60)

  // 「重新寄送（0:42）」的倒數。用 setInterval 而非計算差值，
  // 因為冷卻是後端給的秒數，我們只負責顯示。
  $effect(() => {
    if (cooldown <= 0) return
    const t = setInterval(() => cooldown--, 1000)
    return () => clearInterval(t)
  })

  const mmss = $derived(`0:${String(Math.max(0, cooldown)).padStart(2, '0')}`)

  async function verify(v) {
    busy = true
    err = null
    try {
      await api.joinVerify(app.joinEmail, v)
      verified = true
    } catch (e) {
      err = e.message
      code = ''
    } finally {
      busy = false
    }
  }

  async function createPasskey() {
    busy = true
    err = null
    try {
      await registerPasskey({ email: app.joinEmail })
      notify('帳號已建立', true)
      await refresh()
    } catch (e) {
      err = passkeyError(e)
    } finally {
      busy = false
    }
  }

  async function resend() {
    try {
      const r = await api.joinStart(app.joinEmail)
      cooldown = r.cooldown ?? 60
      code = ''
      err = null
      notify('已重新寄送', true)
    } catch (e) {
      err = e.message
    }
  }
</script>

<div class="flex flex-col min-h-[100dvh]">
  <div class="flex items-center gap-3 px-5 pt-5">
    <button
      onclick={() => (app.authStep = 'join')}
      class="w-9 h-9 min-h-0 rounded-sm bg-surface grid place-items-center"
      aria-label="改信箱">←</button
    >
    <span class="text-body text-fg-muted">改信箱</span>
  </div>

  <div class="flex-1 flex flex-col px-6 pt-6">
    <h1 class="text-head font-bold leading-tight tracking-tight">輸入信箱驗證碼</h1>
    <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
      已寄出 6 位數到 <span class="font-mono">{app.joinEmail}</span>，10 分鐘內有效。
    </p>

    <div class="mt-6">
      <OtpInput bind:value={code} disabled={busy || verified} oncomplete={verify} />
    </div>

    <div class="mt-3 flex items-center justify-between text-body">
      <span class="text-fg-faint">沒收到？檢查垃圾信匣</span>
      {#if cooldown > 0}
        <span class="text-fg-faint">重新寄送（<span class="font-mono">{mmss}</span>）</span>
      {:else}
        <button onclick={resend} class="min-h-0 font-medium text-ok">重新寄送</button>
      {/if}
    </div>

    {#if err}
      <div class="mt-4 p-4 rounded-md bg-bad-bg text-body text-bad-fg">{err}</div>
    {/if}

    <div class="mt-4 p-4 rounded-md bg-surface text-body leading-relaxed text-fg-muted">
      這組驗證碼只確認信箱是你的。通過後才會請你在這支手機建立 Passkey，
      之後登入都不再需要信箱。
    </div>

    <div class="flex-1"></div>

    <button
      onclick={createPasskey}
      disabled={!verified || busy}
      class="w-full py-5 rounded-lg bg-fg text-canvas text-lead font-semibold
             disabled:bg-line-firm disabled:text-fg-faint"
    >
      建立我的 Passkey
    </button>
    <p class="mt-2.5 mb-8 text-center text-label leading-relaxed text-fg-faint">
      {verified ? '信箱已確認，可以建立 Passkey 了' : '填滿 6 位後才會亮起'}
    </p>
  </div>
</div>
