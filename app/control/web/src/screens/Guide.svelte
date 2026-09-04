<script>
  import { app, notify, refresh } from '../lib/state.svelte.js'
  import Copyable from '../components/Copyable.svelte'
  import AllowSheet from '../components/AllowSheet.svelte'
  import IconCheck from '~icons/lucide/check'

  // 設計 1f。分頁順序照設計把「電視」放第一 —— 同戶裝置問題主要出在電視。
  let tab = $state('tv')
  let authorize = $state(false)
  const tabs = [
    { id: 'tv', label: '電視' },
    { id: 'android', label: 'Android' },
    { id: 'ios', label: 'iOS' },
    { id: 'check', label: '檢查' },
  ]

  const s = $derived(app.status)

  // 本層 LAN 的裝置要填 LAN IP，其他樓層填 WAN IP。
  // 這是 split-horizon 的同一套邏輯（見 DECISIONS.md）：本層裝置若填
  // 公網位址，封包會出去再繞回來，而 TP-Link 未必支援 NAT hairpin。
  const dnsIp = $derived(s?.lan_ip ?? s?.wan_ip ?? '未知')
  const isLan = $derived(!!s?.lan_ip)

  // 檢查分頁的三步：授權 → 到裝置上查出口 → 對數字。
  // 第一步由 my_ip_allowed 決定；後兩步要人自己做，面板看不到結果。
  const step = $derived(s?.my_ip_allowed ? 2 : 1)
  // 一般成員的 entries 只有自己加的；別人授權的網路找不到條目，就不顯示天數。
  const myEntry = $derived(s?.entries?.find((e) => e.ip === s?.my_ip) ?? null)
  const daysLeft = (exp) => Math.floor((exp - Date.now() / 1000) / 86400)

  const CHECK_URL = 'https://ifconfig.me'
  async function copyUrl() {
    try {
      await navigator.clipboard.writeText(CHECK_URL)
      notify(`已複製 ${CHECK_URL}`, true)
    } catch {
      notify('請長按網址手動複製')
    }
  }
</script>

<header class="px-5 pt-5 pb-4 bg-surface">
  <h1 class="text-head font-bold">連線教學</h1>
  <!-- 分段式膠囊：選中的那格浮成一張白底，其餘沉在底槽裡。 -->
  <div class="mt-4 grid grid-cols-4 gap-1 p-1 rounded-sm bg-canvas">
    {#each tabs as t (t.id)}
      <button
        onclick={() => (tab = t.id)}
        aria-pressed={tab === t.id}
        class="min-h-0 py-2.5 rounded-[9px] text-item whitespace-nowrap
               {tab === t.id ? 'bg-surface font-semibold shadow-card' : 'text-fg-faint'}"
      >{t.label}</button>
    {/each}
  </div>
</header>

<div class="px-5 pt-3.5 flex flex-col gap-2.5">
  {#if !s?.my_ip_allowed && tab !== 'check'}
    <div class="flex gap-3 p-4 rounded-md bg-bad-bg">
      <span class="w-4 h-4 rounded-chip bg-bad shrink-0 mt-1"></span>
      <div>
        <div class="text-item font-semibold text-bad-fg">這個網路還沒授權</div>
        <p class="mt-1 text-body leading-relaxed text-fg-strong text-pretty">
          設完 DNS 也連不上的話，先到「白名單」授權
          <span class="font-mono">{s?.my_ip}</span>。
        </p>
      </div>
    </div>
  {/if}

  {#if tab === 'tv'}
    <div class="bg-surface rounded-lg p-5">
      <h2 class="text-lead font-semibold">在電視上改 DNS</h2>
      <ol class="mt-4 flex flex-col gap-4">
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">1</span>
          <span class="text-item leading-relaxed pt-0.5">網路設定 → 找到 <b class="font-semibold">IP 設定</b> 或 <b class="font-semibold">DNS 設定</b></span>
        </li>
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">2</span>
          <span class="text-item leading-relaxed pt-0.5">從「自動」改成 <b class="font-semibold">手動</b></span>
        </li>
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">3</span>
          <div class="flex-1 pt-0.5">
            <div class="text-item leading-relaxed">DNS 伺服器填：</div>
            <div class="mt-2.5"><Copyable value={dnsIp} /></div>
          </div>
        </li>
      </ol>

      <p class="mt-4 text-body leading-relaxed text-fg-muted text-pretty">
        改成手動之後，IP／閘道／子網路遮罩通常也要一起填，照原本自動取得的值抄即可。
      </p>

      {#if !isLan}
        <p class="mt-3 text-body leading-relaxed text-watch-fg text-pretty">
          ⚠️ 這是 IP 字面值，<b class="font-semibold">ISP 重撥換 IP 後要回來重設</b>。
        </p>
      {/if}

      <details class="mt-4 pt-3.5 border-t border-line">
        <summary class="text-body font-medium text-ok">為什麼不是私人 DNS？</summary>
        <p class="mt-2.5 text-body leading-relaxed text-fg-muted text-pretty">
          電視系統多數沒有私人 DNS（DoT）選項，只能填明碼 DNS，因此只在家中網路有效。
          在外面看要用手機。
        </p>
      </details>
    </div>

    <div class="bg-surface rounded-lg p-5">
      <div class="flex items-center justify-between gap-3">
        <span class="text-item font-semibold">電視無法個別設定 DNS？</span>
        <span class="font-mono text-label font-medium px-3 py-1.5 rounded-pill bg-bad-bg text-bad-fg">不推薦</span>
      </div>
      <p class="mt-2.5 text-body leading-relaxed text-fg-muted text-pretty">
        在路由器把 DNS 指到 <span class="font-mono">{dnsIp}</span>，家中所有電視、機上盒都不用個別設定。
        代價是整個家的 DNS 都會經過這裡。
      </p>
    </div>

  {:else if tab === 'android'}
    <!-- 手機與電視走同一套：只改「這個 Wi-Fi」的 DNS。私人 DNS 是全機設定，
         換到沒授權的網路整台會連不上網，所以降為不推薦。 -->
    <div class="bg-surface rounded-lg p-5">
      <h2 class="text-lead font-semibold">Android：手動改這個 Wi‑Fi 的 DNS</h2>
      <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
        針對特定的 Wi‑Fi 連線手動調整 IP 設定來更改 DNS。只對這個 Wi‑Fi 生效，其他網路不受影響。
      </p>
      <ol class="mt-4 flex flex-col gap-4">
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">1</span>
          <span class="text-item leading-relaxed pt-0.5">設定 → 網路和網際網路 → Wi‑Fi → 點目前連線網路旁的 <b class="font-semibold">齒輪</b> → 右上角 <b class="font-semibold">鉛筆（編輯）</b></span>
        </li>
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">2</span>
          <span class="text-item leading-relaxed pt-0.5">展開「進階選項」，<b class="font-semibold">IP 設定</b> 從「DHCP」改成 <b class="font-semibold">靜態</b></span>
        </li>
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">3</span>
          <div class="flex-1 pt-0.5">
            <div class="text-item leading-relaxed"><b class="font-semibold">DNS 1</b> 填：</div>
            <div class="mt-2.5"><Copyable value={dnsIp} /></div>
          </div>
        </li>
      </ol>

      <p class="mt-4 text-body leading-relaxed text-fg-muted text-pretty">
        改成靜態之後，IP 位址／閘道通常也要一起填，照原本自動取得的值抄即可。DNS 2 留空，然後儲存。
      </p>

      {#if !isLan}
        <p class="mt-3 text-body leading-relaxed text-watch-fg text-pretty">
          ⚠️ 這是 IP 字面值，<b class="font-semibold">ISP 重撥換 IP 後要回來重設</b>。
        </p>
      {/if}
    </div>

    <div class="bg-surface rounded-lg p-5">
      <div class="flex items-center justify-between gap-3">
        <span class="text-item font-semibold">私人 DNS（網域名）</span>
        <span class="font-mono text-label font-medium px-3 py-1.5 rounded-pill bg-bad-bg text-bad-fg">不推薦</span>
      </div>
      <p class="mt-2.5 text-body leading-relaxed text-fg-muted text-pretty">
        設定 → 網路和網際網路 → 私人 DNS → 選「私人 DNS 供應商主機名稱」填入下方網域名。
        這是全機設定：換到沒授權的網路時，整台手機會連不上網。
      </p>
      <div class="mt-2.5"><Copyable value={s?.dot_host ?? ''} /></div>
      {#if !s?.dot_ready}
        <p class="mt-3 text-body text-watch-fg">⚠️ DoT 尚未就緒（憑證未同步），這個方式暫時不會生效。</p>
      {/if}
    </div>

  {:else if tab === 'ios'}
    <div class="bg-surface rounded-lg p-5">
      <h2 class="text-lead font-semibold">iPhone / iPad：手動改這個 Wi‑Fi 的 DNS</h2>
      <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
        針對特定的 Wi‑Fi 連線手動調整 IP 設定來更改 DNS。只對這個 Wi‑Fi 生效，其他網路不受影響。
      </p>
      <ol class="mt-4 flex flex-col gap-4">
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">1</span>
          <span class="text-item leading-relaxed pt-0.5">設定 → Wi‑Fi → 點目前連線網路右邊的 <b class="font-semibold">ⓘ</b></span>
        </li>
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">2</span>
          <span class="text-item leading-relaxed pt-0.5">往下找到 <b class="font-semibold">設定 DNS</b>，從「自動」改成 <b class="font-semibold">手動</b></span>
        </li>
        <li class="flex gap-3.5">
          <span class="w-6.5 h-6.5 rounded-sm bg-canvas grid place-items-center font-mono text-body font-semibold shrink-0">3</span>
          <div class="flex-1 pt-0.5">
            <div class="text-item leading-relaxed">刪掉原本的伺服器，按「加入伺服器」填：</div>
            <div class="mt-2.5"><Copyable value={dnsIp} /></div>
          </div>
        </li>
      </ol>

      <p class="mt-4 text-body leading-relaxed text-fg-muted text-pretty">
        填完按右上角「儲存」。
      </p>

      {#if !isLan}
        <p class="mt-3 text-body leading-relaxed text-watch-fg text-pretty">
          ⚠️ 這是 IP 字面值，<b class="font-semibold">ISP 重撥換 IP 後要回來重設</b>。
        </p>
      {/if}
    </div>

    <div class="bg-surface rounded-lg p-5">
      <div class="flex items-center justify-between gap-3">
        <span class="text-item font-semibold">安裝 DNS 描述檔</span>
        <span class="font-mono text-label font-medium px-3 py-1.5 rounded-pill bg-bad-bg text-bad-fg">不推薦</span>
      </div>
      <p class="mt-2.5 text-body leading-relaxed text-fg-muted text-pretty">
        描述檔是全機設定：換到沒授權的網路時，整台會連不上網。裝完在
        設定 →「一般」→「VPN 與裝置管理」可以隨時移除。
      </p>
      <button
        onclick={() => (location.href = '/api/dns-profile')}
        disabled={!s?.dot_ready}
        class="mt-3 w-full py-3.5 rounded-md border-[1.5px] border-line-firm text-item font-semibold disabled:opacity-40"
      >下載 DNS 描述檔</button>
      {#if !s?.dot_ready}
        <p class="mt-2 text-body text-watch-fg">⚠️ DoT 尚未就緒（憑證未同步），描述檔暫時無法使用。</p>
      {/if}
    </div>

  {:else}
    <div class="flex items-baseline justify-between px-1">
      <h2 class="text-lead font-semibold">三步確認設定生效</h2>
      <span class="font-mono text-body text-fg-faint">{step} / 3</span>
    </div>

    <!-- 第一步：授權。已授權就收成一行打勾，沒授權就是目前這一步。 -->
    {#if s?.my_ip_allowed}
      <div class="bg-surface rounded-lg p-4 flex items-center gap-3.5">
        <span class="w-9 h-9 rounded-pill bg-ok text-white grid place-items-center shrink-0">
          <IconCheck width="18" height="18" stroke-width="3" />
        </span>
        <div class="min-w-0">
          <div class="text-item font-semibold text-fg-muted">這個網路已經授權</div>
          <div class="mt-0.5 font-mono text-body text-fg-faint truncate">
            {myEntry ? `${s.my_ip} · 還有 ${daysLeft(myEntry.expires_at)} 天` : s?.my_ip}
          </div>
        </div>
      </div>
    {:else}
      <div class="bg-surface rounded-lg p-4 ring-2 ring-ok">
        <div class="flex items-start gap-3.5">
          <span class="w-9 h-9 rounded-pill border-2 border-ok text-ok grid place-items-center font-mono text-item font-semibold shrink-0">1</span>
          <div class="min-w-0 flex-1">
            <div class="text-item font-semibold">先授權這個網路</div>
            <p class="mt-1 text-body leading-relaxed text-fg-muted text-pretty">
              出口 IP <span class="font-mono">{s?.my_ip ?? '取不到'}</span> 還不在白名單裡，設完 DNS 也連不上。
            </p>
            <button
              onclick={() => (authorize = true)}
              class="mt-3 w-full py-3.5 rounded-md bg-ok text-white text-item font-semibold"
            >授權這個網路</button>
          </div>
        </div>
      </div>
    {/if}

    <div class="bg-surface rounded-lg p-4 {step === 2 ? 'ring-2 ring-ok' : ''}">
      <div class="flex items-start gap-3.5">
        <span class="w-9 h-9 rounded-pill grid place-items-center font-mono text-item font-semibold shrink-0
                     {step === 2 ? 'border-2 border-ok text-ok' : 'bg-canvas text-fg-faint'}">2</span>
        <div class="min-w-0 flex-1">
          <div class="text-item font-semibold">在這台裝置查一下出口位址</div>
          <p class="mt-1 text-body leading-relaxed text-fg-muted text-pretty">
            開瀏覽器到 <span class="font-mono">ifconfig.me</span>，畫面上那一行就是答案。
          </p>
          <div class="mt-3 flex gap-2">
            <a
              href={CHECK_URL}
              target="_blank"
              rel="noopener"
              class="flex-1 grid place-items-center py-3.5 rounded-md bg-ok text-white text-item font-semibold"
            >點此開啟網站</a>
            <button
              onclick={copyUrl}
              class="px-4 py-3.5 rounded-md border-[1.5px] border-line-firm text-item font-semibold"
            >複製網址</button>
          </div>
        </div>
      </div>
    </div>

    <div class="bg-surface rounded-lg p-4">
      <div class="flex items-start gap-3.5">
        <span class="w-9 h-9 rounded-pill bg-canvas text-fg-faint grid place-items-center font-mono text-item font-semibold shrink-0">3</span>
        <div class="min-w-0 flex-1">
          <div class="text-item font-semibold">數字一樣即完成！</div>
          <div class="mt-3 flex items-center justify-between gap-3 px-4 py-3.5 rounded-md bg-canvas">
            <span class="text-body text-fg-faint shrink-0">IP Address</span>
            <span class="font-mono text-lead font-medium truncate">{s?.wan_ip ?? '未知'}</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 三種「還是不一樣」裡最常見的都是快取：Wi-Fi 重連拿新設定、等舊紀錄過期。 -->
    <div class="rounded-lg p-4 bg-watch-bg">
      <div class="text-item font-semibold text-watch-fg">剛改完設定卻還是不一樣？</div>
      <ol class="mt-2 flex flex-col gap-1.5">
        {#each [
          '把 Wi‑Fi 關掉再開，或忘記這個網路重新連。',
          '等 30 秒讓舊紀錄過期，再重新整理網頁。',
          '若還是不一樣，請檢查是否有輸入錯誤。',
        ] as line, i (line)}
          <li class="flex gap-3 text-body leading-relaxed text-watch-fg">
            <span class="font-mono text-fg-faint shrink-0">{i + 1}</span>
            <span class="text-pretty">{line}</span>
          </li>
        {/each}
      </ol>
    </div>
  {/if}
</div>

<AllowSheet open={authorize} onclose={() => (authorize = false)} ondone={refresh} />
