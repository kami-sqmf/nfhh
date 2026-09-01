// 推送通知的瀏覽器端。
//
// 「能不能推」用特徵偵測（`PushManager` 在不在）—— 比認 UA 可靠，
// 也天生涵蓋「加到主畫面但關掉了『開啟為網頁 App』」那個坑。
// 「為什麼不能」才認 iOS，單純為了決定顯示哪張說明（設計 3b）。

import { api } from './api.js'

/** 這個環境到底推不推得動。 */
const canPush = () =>
  'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window

/** 從主畫面圖示開的（iOS 的 web app 模式，或 Android 的已安裝狀態）。 */
const isStandalone = () =>
  window.matchMedia?.('(display-mode: standalone)').matches || navigator.standalone === true

/** iPhone / iPad。iPadOS 13 起 UA 會裝成 Mac，靠觸控點數才分得出來。 */
const isIOS = () =>
  /iPad|iPhone|iPod/.test(navigator.userAgent) ||
  (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1)

/**
 * 現在該給使用者看什麼。
 *
 *   'ready'     —— 推得動，還沒訂閱（設計 3a）
 *   'on'        —— 已經訂閱了
 *   'homescreen'—— iOS 但還沒加到主畫面（設計 3b）
 *   'blocked'   —— 使用者拒絕過權限，只能自己去瀏覽器設定改
 *   'unsupported'—— 這個瀏覽器就是不支援，沒有話可以說
 */
export async function pushState() {
  if (!canPush()) {
    // iOS 分頁沒有 PushManager —— 不是壞掉，是還差「加到主畫面」那一步
    if (isIOS() && !isStandalone()) return 'homescreen'
    return 'unsupported'
  }
  if (Notification.permission === 'denied') return 'blocked'

  const reg = await navigator.serviceWorker.ready
  const sub = await reg.pushManager.getSubscription()
  if (!sub) return 'ready'

  // 跟面板對一次帳。瀏覽器端的訂閱是本機物件 —— 別台裝置在設定裡把這台
  // 停掉之後它照樣存在，光看它會一直說「已開啟」，但推播早就送不到了。
  try {
    const { registered } = await api.pushCheck(sub.endpoint)
    if (!registered) {
      // 面板已經沒有這筆，本機這個殼留著只會繼續騙人
      await sub.unsubscribe().catch(() => {})
      return 'ready'
    }
  } catch {
    // 問不到（離線、面板掛了）就維持現狀 —— 網路不通不代表被撤掉
  }
  return 'on'
}

/** base64url 的 VAPID 公鑰 → applicationServerKey 要的 Uint8Array。 */
function decodeKey(b64) {
  const padded = (b64 + '='.repeat((4 - (b64.length % 4)) % 4)).replace(/-/g, '+').replace(/_/g, '/')
  return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0))
}

const b64 = (buf) =>
  btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '')

/**
 * 開啟通知。**必須由使用者的點擊直接呼叫** —— iOS 要求權限請求來自
 * 明確的手勢，隔一層 await 之後才問會被直接拒絕。
 *
 * 回傳 true = 成功訂閱；false = 使用者按了「不允許」。
 */
export async function enablePush(label) {
  const reg = await navigator.serviceWorker.register('/sw.js')
  await navigator.serviceWorker.ready

  const permission = await Notification.requestPermission()
  if (permission !== 'granted') return false

  const { key } = await api.pushKey()
  const sub =
    (await reg.pushManager.getSubscription()) ??
    (await reg.pushManager.subscribe({
      // 少了這個旗標瀏覽器不給訂閱：不能拿推送當靜默的背景通道
      userVisibleOnly: true,
      applicationServerKey: decodeKey(key),
    }))

  await api.pushSubscribe({
    endpoint: sub.endpoint,
    p256dh: b64(sub.getKey('p256dh')),
    auth: b64(sub.getKey('auth')),
    label: label || null,
  })
  return true
}

/** 關掉這台裝置的通知。兩邊都要退，只退一邊會留下白推的殭屍訂閱。 */
export async function disablePush() {
  const reg = await navigator.serviceWorker.ready
  const sub = await reg.pushManager.getSubscription()
  if (!sub) return

  // 先告訴面板再退訂 —— 反過來的話 endpoint 已作廢，面板那筆會留著白推
  await api.pushUnsubscribeSelf(sub.endpoint).catch(() => {})
  await sub.unsubscribe().catch(() => {})
}
