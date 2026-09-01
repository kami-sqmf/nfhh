<script>
  // 原始信件檢視。從舊面板搬回來的 —— 改寫成 Svelte 時我漏掉了這塊，
  // 而它的安全性質是刻意設計的，不是順手加的：
  //
  //   sandbox=""        空值代表全部限制生效：不能執行 script、獨立來源、
  //                     不能導覽父框架。信件內容是外部來的，一律當敵意處理。
  //   CSP img-src data: 預設封鎖遠端圖片。信件裡的圖片多半是追蹤像素，
  //                     載入等於告訴寄件者「這個信箱是活的、他在幾點讀了信、
  //                     以及他的 IP」。要看再自己按。
  //   referrerpolicy    連 Referer 都不給。
  //   <base target=_blank>  信裡的連結不會把 iframe 自己導走。
  //
  // 用 srcdoc 屬性指派而非字串拼接，省去逸出問題。
  import Sheet from './Sheet.svelte'
  import { ago } from '../lib/time.js'

  let { mail = null, onclose } = $props()

  let allowImg = $state(false)
  let showText = $state(false)

  const srcdoc = $derived(
    mail?.html
      ? `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; ` +
        `style-src 'unsafe-inline'; img-src ${allowImg ? 'https: data:' : 'data:'}; ` +
        `font-src data:;"><base target="_blank">${mail.html}`
      : ''
  )
</script>

<Sheet open={!!mail} {onclose} title="原始信件">
  {#if mail}
    <div class="mt-3 text-body text-fg-muted">
      <div class="text-item font-semibold text-fg">{mail.subject || '(無主旨)'}</div>
      <div class="mt-1 font-mono text-label truncate">{mail.sender || '未知寄件者'}</div>
      <div class="mt-0.5 text-label">{ago(mail.received_at)}</div>
    </div>

    {#if mail.verified === false}
      <div class="mt-3 p-4 rounded-md bg-watch-bg text-label leading-relaxed text-fg-strong text-pretty">
        ⚠️ 這封信沒有通過寄件者驗證 —— 寄件網域的 DKIM 簽章不屬於已知平台。
        任何人都能寄信到這個信箱，收到看起來像官方的信不代表就是官方寄的。
        把驗證碼拿去用之前，請先確認你真的正在登入。
      </div>
    {/if}

    {#if mail.html}
      <div class="mt-4 flex items-center justify-between gap-3 p-3 rounded-sm bg-canvas">
        <span class="text-label leading-relaxed text-fg-muted text-pretty">
          {allowImg ? '遠端圖片已放行' : '已封鎖遠端圖片（避免追蹤像素洩漏你已讀信與 IP）'}
        </span>
        <button
          onclick={() => (allowImg = !allowImg)}
          class="shrink-0 px-3 py-2 min-h-0 rounded-sm border-[1.5px] border-line-firm text-label font-medium"
        >{allowImg ? '停止載入' : '載入圖片'}</button>
      </div>

      <iframe
        title="信件內容"
        sandbox=""
        referrerpolicy="no-referrer"
        {srcdoc}
        class="mt-2 w-full h-[50vh] bg-white rounded-sm border border-line"
      ></iframe>
    {/if}

    {#if mail.body}
      <button
        onclick={() => (showText = !showText)}
        class="mt-3 w-full py-3 min-h-0 rounded-sm border-[1.5px] border-line-firm text-body font-medium"
      >{showText ? '收合純文字內文' : '看純文字內文'}</button>
      {#if showText}
        <pre class="mt-2 p-3 rounded-sm bg-canvas font-mono text-label leading-relaxed
                    whitespace-pre-wrap break-words max-h-[40vh] overflow-y-auto">{mail.body}</pre>
      {/if}
    {/if}

    {#if mail.links?.length}
      <div class="mt-3">
        <div class="text-label text-fg-faint">信中的連結</div>
        <!-- 錨點文字一律用完整網址：顯示文字與實際目的地不同是釣魚的基本手法 -->
        <div class="mt-1.5 flex flex-col gap-1.5">
          {#each mail.links.slice(0, 5) as u (u)}
            <a href={u} target="_blank" rel="noopener noreferrer"
               class="font-mono text-label text-ok break-all">{u}</a>
          {/each}
        </div>
      </div>
    {/if}

    {#if !mail.html && !mail.body}
      <p class="mt-4 text-body text-fg-faint">這封信沒有保留內文。</p>
    {/if}
  {/if}
</Sheet>
