// 時間格式化。全部用「相對」而非絕對 —— 面板要回答的是
// 「還剩多久」「多久以前」，不是「幾點幾分」。

const now = () => Date.now() / 1000

/** 「3 分鐘前」。驗證碼的時效很短，分鐘級的精度是必要的。 */
export function ago(ts) {
  const m = Math.round((now() - ts) / 60)
  if (m < 1) return '剛剛'
  if (m < 60) return `${m} 分鐘前`
  if (m < 1440) return `${Math.round(m / 60)} 小時前`
  return `${Math.round(m / 1440)} 天前`
}

/**
 * 「剩 6d 23h」。刻意用 d/h 而非「天」「小時」：
 * 這個值旁邊都是等寬字體的 IP 與時間，混用中文單位會讓那一行跳動。
 */
export function left(expiresAt) {
  const s = Math.max(0, expiresAt - now())
  const d = Math.floor(s / 86400)
  const h = Math.floor((s % 86400) / 3600)
  if (d > 0) return `${d}d ${h}h`
  if (h > 0) return `${h}h`
  return `${Math.max(1, Math.floor(s / 60))}m`
}

/** 到期前一天內要變色示警 */
export const expiringSoon = (expiresAt) => expiresAt - now() < 86400

/** 「2 年前」。用於 Cloudflare 的驗證時間，精度到年月就夠。 */
export function since(ts) {
  const days = (now() - ts) / 86400
  if (days < 1) return '今天'
  if (days < 30) return `${Math.round(days)} 天前`
  if (days < 365) return `${Math.round(days / 30)} 個月前`
  return `${Math.floor(days / 365)} 年前`
}

/** 只有時分。驗證碼卡上跟相對時間並列，回答「這是幾點寄來的」。 */
export function clock(ts) {
  return new Date(ts * 1000).toLocaleTimeString('zh-TW', {
    hour: '2-digit', minute: '2-digit', hour12: false,
  })
}

/** 稽核紀錄用的時刻。同一天只顯示時間，跨天才帶日期。 */
export function stamp(ts) {
  const d = new Date(ts * 1000)
  const today = new Date()
  const sameDay = d.toDateString() === today.toDateString()
  const hm = d.toLocaleTimeString('zh-TW', { hour: '2-digit', minute: '2-digit', hour12: false })
  return sameDay ? hm : `${d.getMonth() + 1}/${d.getDate()} ${hm}`
}
