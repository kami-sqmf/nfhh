// 全域狀態。
//
// 只放「整個 app 都要知道」的東西：登入狀態、目前分頁、以及一個
// 共用的訊息列。各畫面自己的資料留在各畫面裡，不往這裡堆。

import { api } from './api.js'
import { egressIpv4 } from './ip.js'

/**
 * 邀請函的連結長 `/join/<token>`。權杖是憑據 —— 讀完立刻把網址換乾淨，
 * 免得它跟著瀏覽紀錄、分享按鈕與螢幕截圖到處跑。
 *
 * 格式先擋一次：SPA 的 fallback 會把任何路徑都回成 index.html，
 * 不篩的話 `/join` 底下隨便一個字串都會被送去後端試兌換。
 */
function takeInviteToken() {
  const m = location.pathname.match(/^\/join\/([0-9a-f]{32,128})\/?$/i)
  if (!m) return null
  history.replaceState(null, '', '/')
  return m[1]
}

const inviteToken = takeInviteToken()

export const app = $state({
  /** 後端 /api/status 的完整回應。null = 還沒載入。 */
  status: null,
  /** 底部分頁：home | allow | codes | guide | admin */
  tab: 'home',
  /** 管理分頁的子頁：null = 管理首頁 */
  sub: null,
  /** 未登入時的流程：login | join | joincode | invited */
  authStep: inviteToken ? 'invited' : 'login',
  /** 加入流程進行中的信箱，在三個畫面之間傳遞 */
  joinEmail: '',
  /** 邀請函連結帶進來的權杖。null = 從一般入口進來的 */
  inviteToken,
  msg: null, // { text, ok }
})

export function notify(text, ok = false) {
  app.msg = { text, ok }
  // 成功訊息自動退場；錯誤留著，因為使用者可能需要照著它做事
  if (ok) setTimeout(() => { if (app.msg?.text === text) app.msg = null }, 4000)
}

export const fail = (e) => notify(e.message || String(e), false)

export async function refresh() {
  try {
    // 出口 IPv4 要在瀏覽器端問，後端問不到（見 lib/ip.js）。整頁只問一次，
    // 之後的 refresh 拿的是同一顆已解析的 promise。
    app.status = await api.status(await egressIpv4())
    // 角色可能被降權 —— 分頁要即時消失，不能停在一個已經沒權限的畫面
    if (app.tab === 'admin' && !app.status.is_admin) {
      app.tab = 'home'
      app.sub = null
    }
  } catch (e) {
    fail(e)
  }
}

export function go(tab, sub = null) {
  app.tab = tab
  app.sub = sub
  app.msg = null
  window.scrollTo(0, 0)
}
