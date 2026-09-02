<script>
  import { notify } from '../lib/state.svelte.js'
  import { ago, clock } from '../lib/time.js'
  import Pill from './Pill.svelte'
  import IconExternal from '~icons/lucide/external-link'
  import PlatformMark from './PlatformMark.svelte'

  // showMailbox：設計 1b 的首頁卡顯示收件信箱，1e 的清單卡不顯示 ——
  // 後者右側有兩顆按鈕，硬塞第三項只會把信箱截成沒有意義的片段。
  let { mail, platformName, platformColor = null, showMailbox = false,
        ondelete, onview } = $props()

  // verified 有三種值，不是兩種：
  //   true  → 通過
  //   false → 未通過（琥珀，觀察期仍顯示）
  //   null  → v5 之前的舊信，當時還沒有這個欄位。這是「無驗證資訊」
  //           不是「未通過」—— 1a：灰永不與紅混用。
  const state = $derived(
    mail.verified === true ? { tone: 'ok', text: '✔ 已認證' }
    : mail.verified === false ? { tone: 'watch', text: '觀察期 · 未通過' }
    : { tone: 'none', text: '無驗證資訊' }
  )

  // 按鈕下方一定露出目的網域：品牌卡片是背書，使用者要看得到背書的是哪裡。
  // URL.hostname 給的是 punycode，同形字網域不會被畫成本尊。
  const host = (u) => { try { return new URL(u).hostname } catch { return u } }

  async function copy() {
    try {
      await navigator.clipboard.writeText(mail.code)
      notify(`已複製 ${mail.code}`, true)
    } catch {
      notify('請長按數字手動複製')
    }
  }
</script>

<div
  class="bg-surface rounded-lg p-5 {mail.verified === true ? 'ring-2 ring-ok' : ''}"
>
  <div class="flex items-center justify-between gap-2">
    <div class="flex items-center gap-2.5 min-w-0">
      <PlatformMark code={mail.platform ?? ""} color={platformColor} name={platformName} size="md" />
      <span class="text-item font-semibold truncate">{platformName}</span>
    </div>
    <Pill tone={state.tone}>{state.text}</Pill>
  </div>

  {#if mail.code}
    <button
      onclick={copy}
      class="mt-3 w-full flex items-end justify-between gap-3 text-left min-h-0 active:opacity-70"
    >
      <span class="font-mono font-semibold leading-none tracking-[0.06em] text-code">{mail.code}</span>
      <span class="text-body font-medium text-ok pb-1 shrink-0">點一下複製</span>
    </button>
  {:else if mail.primary_link}
    <!-- 沒有碼的信（Netflix 的「暫時存取碼」就是這種）—— 碼在連結後面。
         按鈕直接跳出去平台取碼，不要求使用者自己去翻原始信件。 -->
    <a
      href={mail.primary_link}
      target="_blank"
      rel="noopener noreferrer"
      class="mt-3 w-full py-4 rounded-md bg-ok text-white text-lead font-semibold
             flex items-center justify-center gap-2"
    >
      <IconExternal width="18" height="18" />
      取得存取碼
    </a>
    <p class="mt-2 text-label leading-relaxed text-fg-muted text-pretty">
      會開啟 <span class="font-mono">{host(mail.primary_link)}</span>。這封信沒有直接附上號碼，要到平台的頁面取得。
    </p>
  {:else}
    <p class="mt-3 text-body text-fg-faint">
      {#if mail.verified !== true}
        這封信未通過寄件者驗證，連結不會顯示。請開原始信件自行判斷。
      {:else}
        這封信沒有可用的號碼或連結，請開原始信件查看。
      {/if}
    </p>
  {/if}

  <div class="mt-3 flex items-center justify-between gap-3">
    <!-- 設計 1b 的 metadata 行：時刻 · 相對時間 · 收件信箱。
         收件信箱要露出來，因為多平台共用面板時它是唯一能分辨
         「這封是寄到哪個信箱」的線索。 -->
    <span class="flex items-center gap-1.5 text-label text-fg-faint min-w-0">
      <span class="font-mono shrink-0">{clock(mail.received_at)}</span>
      <span>·</span>
      <span class="shrink-0">{ago(mail.received_at)}</span>
      {#if showMailbox && mail.recipient}
        <span>·</span>
        <span class="font-mono truncate">{mail.recipient}</span>
      {/if}
    </span>
    <div class="flex gap-2 shrink-0">
      {#if onview}
        <button
          onclick={() => onview(mail)}
          class="px-3 py-2.5 min-h-0 rounded-sm border-[1.5px] border-line-firm text-body font-medium"
        >原始信件</button>
      {/if}
      {#if ondelete}
        <button
          onclick={() => ondelete(mail.id)}
          class="px-3 py-2.5 min-h-0 rounded-sm border-[1.5px] border-bad/35 text-body font-medium text-bad-fg"
        >刪除</button>
      {/if}
    </div>
  </div>
</div>
