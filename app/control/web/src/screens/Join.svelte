<script>
  import { app, notify, fail } from '../lib/state.svelte.js'
  import { api } from '../lib/api.js'

  let email = $state(app.joinEmail)
  let busy = $state(false)
  let err = $state(null)

  async function next() {
    const v = email.trim().toLowerCase()
    if (!v.includes('@')) return (err = '請輸入完整的 Email 位址')
    busy = true
    err = null
    try {
      await api.joinStart(v)
      app.joinEmail = v
      app.authStep = 'joincode'
    } catch (e) {
      // 這頁的錯誤留在原地（設計 1k 的紅色區塊），不用飄過去的訊息列 ——
      // 使用者要照著它改輸入，訊息不能自己消失。
      err = e.message
    } finally {
      busy = false
    }
  }
</script>

<div class="flex flex-col min-h-[100dvh]">
  <div class="flex items-center gap-3 px-5 pt-5">
    <button
      onclick={() => (app.authStep = 'login')}
      class="w-9 h-9 min-h-0 rounded-sm bg-surface grid place-items-center"
      aria-label="回登入">←</button
    >
    <span class="text-body text-fg-muted">回登入</span>
  </div>

  <div class="flex-1 flex flex-col px-6 pt-6">
    <h1 class="text-head font-bold leading-tight tracking-tight">用 Email 加入</h1>
    <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
      輸入管理員登記的位址，必須完全相符。位址不會過期，除非管理員撤銷。
    </p>

    <div class="mt-6 p-5 bg-surface rounded-md ring-2 ring-fg">
      <span class="font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
        Email
      </span>
      <input
        bind:value={email}
        type="email"
        inputmode="email"
        autocomplete="email"
        autocapitalize="off"
        spellcheck="false"
        placeholder="you@example.com"
        onkeydown={(e) => e.key === 'Enter' && next()}
        class="mt-2 w-full bg-transparent font-mono text-title font-medium outline-none"
      />
    </div>

    {#if err}
      <div class="mt-4 flex gap-3 p-4 rounded-md bg-bad-bg">
        <span class="w-3.5 h-3.5 rounded-chip bg-bad shrink-0 mt-1"></span>
        <div>
          <div class="text-body font-semibold text-bad-fg">無法繼續</div>
          <p class="mt-1 text-body leading-relaxed text-fg-strong">{err}</p>
        </div>
      </div>
    {/if}

    <div class="flex-1"></div>

    <button
      onclick={next}
      disabled={busy || !email.trim()}
      class="w-full py-5 rounded-lg bg-fg text-canvas text-lead font-semibold
             disabled:bg-line-firm disabled:text-fg-faint"
    >
      {busy ? '寄送中…' : '下一步'}
    </button>
    <p class="mt-2.5 mb-8 text-center text-label leading-relaxed text-fg-faint">
      下一步會寄一組驗證碼到這個信箱
    </p>
  </div>
</div>
