<script>
  // 平台標記。
  //
  // 兩層：已知平台用 streamline-logos 的品牌圖標（建置時內聯，不打 API）；
  // 未知平台退回「字母 + 顏色」。
  //
  // 為什麼要有退路：平台是資料驅動的 —— 丟一個 .list 進 domain-set 就多一個
  // 平台。若只認品牌圖標，每加一個平台都得回來改前端；有了退路，新平台當下
  // 就有可分辨的標記，之後想補品牌圖標再補。
  //
  // 顏色優先取 .list 檔宣告的 `# platform-color:`，沒宣告才從代號推導。
  import IconNetflix from '~icons/streamline-logos/netflix-logo-block'
  import IconDisneyPlus from '~icons/streamline-logos/disney-plus-logo-block'

  let { code = '', name = '', color = null, size = 'md' } = $props()

  // 靜態對照：圖標必須在建置時就決定，不能由伺服器的字串動態選。
  const LOGOS = { netflix: IconNetflix, disneyplus: IconDisneyPlus }
  const Logo = $derived(LOGOS[code])

  // 簡單的字串雜湊 → 0–359。不需要密碼學強度，只要穩定且分散。
  const hue = $derived([...code].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 7))
  const bg = $derived(color ?? `oklch(0.55 0.15 ${hue})`)
  const letter = $derived((name || code).trim().charAt(0).toUpperCase() || '?')

  const px = { sm: 16, md: 22, lg: 32 }
  const box = {
    sm: 'w-4 h-4 text-[9px] rounded-[4px]',
    md: 'w-5.5 h-5.5 text-[11px] rounded-chip',
    lg: 'w-8 h-8 text-body rounded-sm',
  }
</script>

{#if Logo}
  <!-- 品牌圖標自己帶配色，不套背景 -->
  <Logo width={px[size]} height={px[size]} class="shrink-0" aria-hidden="true" />
{:else}
  <span
    class="grid place-items-center shrink-0 font-semibold text-white select-none {box[size]}"
    style="background: {bg}"
    aria-hidden="true"
  >{letter}</span>
{/if}
