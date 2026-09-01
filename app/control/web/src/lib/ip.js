// 問出「這個網路的對外公網 IPv4」。
//
// 為什麼不能用連線來源：面板走 Cloudflare Tunnel，後端看到的
// `cf-connecting-ip` 是**開面板這台裝置**連到 Cloudflare 用的位址。手機開著
// IPv6 的話那是一個 /128，而白名單閘門是精準比對單一位址
// （config/nft/nfhh.nft 的 clients_v4 / clients_v6 都沒有 flags interval）。
// 授權一個 /128 的後果有三個，每一個都會讓人以為「面板壞了」：
//
//   1. 同一戶的電視照文件把 DNS 填成出口 IP，走的是 IPv4，來源是那戶的
//      WAN IPv4 —— 不在 clients_v4 裡，照樣被 drop。
//   2. IPv6 沒有 NAT，一個位址只代表一台裝置的一張介面。
//   3. SLAAC 的臨時位址會定期輪替，過幾天連授權的那台自己都會失效。
//
// IPv4 因為 NAT，一筆就代表整戶 —— 所以授權對象一律取 IPv4。
//
// 只有「從那個網路送出去的請求」對方才看得到那一戶的公網 IPv4，因此這件事
// 只能在瀏覽器端做，而且對象必須是只有 A 記錄的服務。
//
// ⚠️ 這會把家人的公網 IP 送給第三方。範圍跟 scripts/apply-config.sh 一樣
//    （那支在伺服器端問同一批服務），沒有多洩漏什麼；換服務前想清楚這點。
//    ifconfig.me 不在清單裡 —— 它不給 CORS 標頭，瀏覽器讀不到回應。

const SOURCES = ['https://api.ipify.org', 'https://ipv4.icanhazip.com']
const TIMEOUT_MS = 3000
const V4 = /^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$/

async function ask(url, signal) {
  const res = await fetch(url, { signal, cache: 'no-store', referrerPolicy: 'no-referrer' })
  if (!res.ok) throw new Error(`${url} ${res.status}`)
  const ip = (await res.text()).trim()
  // 服務掛掉時常常回一頁 HTML 而不是錯誤碼，所以格式要自己驗
  if (!V4.test(ip)) throw new Error(`${url} 回的不是 IPv4`)
  return ip
}

let pending = null

/**
 * 這個網路的公網 IPv4，問不到就 null（呼叫端要能接受這件事）。
 *
 * 整頁只問一次 —— 失敗也記住。每次 refresh 都重試的話，網路擋掉這些服務的
 * 人會被每一次操作都拖上三秒。要重新偵測就重新整理頁面。
 */
export function egressIpv4() {
  if (pending) return pending
  const ctl = new AbortController()
  const timer = setTimeout(() => ctl.abort(), TIMEOUT_MS)
  pending = Promise.any(SOURCES.map((u) => ask(u, ctl.signal)))
    .catch(() => null)
    .finally(() => {
      clearTimeout(timer)
      ctl.abort() // 已經有答案了，另一家不必再等
    })
  return pending
}
