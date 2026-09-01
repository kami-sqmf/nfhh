<script>
  import { app, notify } from '../lib/state.svelte.js'
  import Copyable from '../components/Copyable.svelte'

  // 設計 1f。分頁順序照設計把「電視」放第一 —— 同戶裝置問題主要出在電視。
  let tab = $state('tv')
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
</script>

<header class="px-5 pt-5 bg-surface">
  <h1 class="text-head font-bold">連線教學</h1>
  <div class="mt-4 flex gap-6 overflow-x-auto">
    {#each tabs as t (t.id)}
      <button
        onclick={() => (tab = t.id)}
        class="pb-2.5 min-h-0 text-item whitespace-nowrap
               {tab === t.id ? 'font-semibold border-b-[2.5px] border-fg' : 'text-fg-faint'}"
      >{t.label}</button>
    {/each}
  </div>
</header>

<div class="px-5 pt-3.5 flex flex-col gap-2.5">
  {#if !s?.my_ip_allowed}
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
          手機請用 iOS／Android 分頁的網域名方式，不受影響。
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
    <div class="bg-surface rounded-lg p-5">
      <h2 class="text-lead font-semibold">Android：私人 DNS</h2>
      <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
        走網域名而不是 IP，ISP 重撥換 IP 也不受影響。
      </p>
      <ol class="mt-4 flex flex-col gap-4">
        <li class="text-item leading-relaxed">設定 → 網路和網際網路 → <b class="font-semibold">私人 DNS</b></li>
        <li class="text-item leading-relaxed">選「私人 DNS 供應商主機名稱」，填入：</li>
      </ol>
      <div class="mt-2.5"><Copyable value={s?.dot_host ?? ''} /></div>
      {#if !s?.dot_ready}
        <p class="mt-3 text-body text-watch-fg">⚠️ DoT 尚未就緒（憑證未同步），這個方式暫時不會生效。</p>
      {/if}
    </div>

  {:else if tab === 'ios'}
    <div class="bg-surface rounded-lg p-5">
      <h2 class="text-lead font-semibold">iPhone / iPad</h2>
      <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
        iOS 沒有內建的 DoT 欄位，要安裝一個描述檔。裝完在
        設定 →「一般」→「VPN 與裝置管理」可以隨時移除。
      </p>
      <button
        onclick={() => (location.href = '/api/dns-profile')}
        disabled={!s?.dot_ready}
        class="mt-4 w-full py-4 rounded-md bg-fg text-canvas text-item font-semibold disabled:opacity-40"
      >下載 DNS 描述檔</button>
      {#if !s?.dot_ready}
        <p class="mt-2 text-body text-watch-fg">⚠️ DoT 尚未就緒（憑證未同步），描述檔暫時無法使用。</p>
      {/if}
    </div>

  {:else}
    <div class="bg-surface rounded-lg p-5">
      <h2 class="text-lead font-semibold">確認設定生效了</h2>
      <p class="mt-2 text-body leading-relaxed text-fg-muted text-pretty">
        在裝置的瀏覽器開 <span class="font-mono">ifconfig.me</span>，對照它顯示的 IP：
      </p>
      <div class="mt-3.5 flex flex-col divide-y divide-line">
        {#each [
          ['顯示 ' + (s?.wan_ip ?? '本站的 IP'), '設定成功', 'text-ok-fg'],
          ['顯示其他 IPv4', 'DNS 沒生效，或走了系統內建的加密 DNS', 'text-watch-fg'],
          ['顯示 IPv6（含冒號）', '走 IPv6 直連繞過了', 'text-watch-fg'],
          ['連不上', '這個網路還沒授權', 'text-watch-fg'],
        ] as [what, why, tone] (what)}
          <div class="flex justify-between gap-4 py-2.5 text-body">
            <span class="shrink-0">{what}</span>
            <span class="text-right {tone}">{why}</span>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>
