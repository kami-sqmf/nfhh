<script>
  import { app, notify, refresh } from '../lib/state.svelte.js'
  import { api, registerPasskey, passkeyError } from '../lib/api.js'
  import PlatformMark from '../components/PlatformMark.svelte'

  // 從邀請函的連結進來的人。連結已經證明了信箱是他的，所以這頁沒有
  // 輸入欄位，也沒有驗證碼 —— 只剩下「建立 Passkey」這一步。
  //
  // 連結失效時不把人丟在死路上：下面永遠留著一條回「用 Email 加入」的路，
  // 那條完全沒動，重新收一組碼就能繼續。
  let invite = $state(null)
  let busy = $state(false)
  let err = $state(null)

  const platforms = $derived(app.status?.platforms ?? [])

  async function open() {
    try {
      invite = await api.joinInvite(app.inviteToken)
    } catch (e) {
      err = e.message
    }
  }

  async function createPasskey() {
    busy = true
    err = null
    try {
      await registerPasskey({ email: invite.email })
      notify('帳號已建立', true)
      await refresh()
    } catch (e) {
      err = passkeyError(e)
    } finally {
      busy = false
    }
  }

  $effect(() => { open() })
</script>

<div class="flex flex-col min-h-[100dvh] px-6">
  <div class="pt-10">
    <div class="font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
      邀請函
    </div>
    <h1 class="mt-2 text-head font-bold leading-tight tracking-tight">建立你的帳號</h1>
    <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
      這封邀請函寄到你的信箱，信箱已經確認過了 —— 不用再輸入驗證碼。
    </p>
  </div>

  {#if invite}
    <div class="mt-6 p-5 bg-surface rounded-md">
      <span class="font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
        你的信箱
      </span>
      <div class="mt-1 font-mono text-title font-medium truncate">{invite.email}</div>

      <div class="mt-4 pt-4 border-t border-line">
        <span class="font-mono text-micro font-medium tracking-widest uppercase text-fg-faint">
          可用服務
        </span>
        <div class="mt-2 flex flex-wrap items-center gap-1.5">
          {#each invite.platforms as c (c)}
            {@const p = platforms.find((x) => x.code === c)}
            <span class="flex items-center gap-1 px-2 py-0.5 rounded-pill bg-ok-bg text-ok-fg text-meta">
              <PlatformMark code={c} name={p?.name ?? c} color={p?.color} size="sm" />
              {p?.name ?? c}
            </span>
          {:else}
            <!-- 登記時沒指定平台是合法的。講明白，否則建完帳號看到空的
                 驗證碼分頁會以為是壞掉了。 -->
            <span class="text-meta text-watch-fg">尚未指定 · 建立帳號後請管理員開通</span>
          {/each}
        </div>
      </div>
    </div>
  {:else if !err}
    <div class="mt-6 p-5 bg-surface rounded-md text-body text-fg-faint">確認邀請中…</div>
  {/if}

  {#if err}
    <div class="mt-4 flex gap-3 p-4 rounded-md bg-bad-bg">
      <span class="w-3.5 h-3.5 rounded-chip bg-bad shrink-0 mt-1"></span>
      <div>
        <div class="text-body font-semibold text-bad-fg">無法繼續</div>
        <p class="mt-1 text-body leading-relaxed text-fg-strong">{err}</p>
      </div>
    </div>
  {/if}

  <div class="flex-1 min-h-[24px]"></div>

  <button
    onclick={createPasskey}
    disabled={!invite || busy}
    class="w-full py-5 rounded-lg bg-fg text-canvas text-lead font-semibold
           disabled:bg-line-firm disabled:text-fg-faint"
  >
    {busy ? '建立中…' : '建立我的 Passkey'}
  </button>
  <p class="mt-2.5 text-center text-label leading-relaxed text-fg-faint">
    用這台裝置的指紋或臉部辨識，不用另設密碼
  </p>

  <button
    onclick={() => {
      // 位址帶過去，那頁就不必再打一次（打錯會被當成沒被邀請）
      app.flowEmail = invite?.email ?? app.flowEmail
      app.authStep = 'join'
    }}
    class="mt-4 mb-8 w-full py-2.5 min-h-0 text-body text-fg-faint"
  >
    連結有問題？改用 Email 加入
  </button>
</div>
