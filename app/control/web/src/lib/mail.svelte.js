// 三個畫面的信件清單邏輯：首頁、驗證碼分頁、管理收件匣。
// 差別只有打哪支端點，其餘（輪詢的 single-flight、點開才取全文）一字不差 ——
// 抄三份的下場是修一處忘兩處，所以放在這裡共用一份。
import { api } from './api.js'
import { fail } from './state.svelte.js'

// fetchList 是回清單摘要的那支：api.mails（自己的）或 api.inbox（管理）。
export function mailList(fetchList) {
  let mails = $state([])
  let viewing = $state(null)

  // 慢回應不能跟下一次 interval 疊加：上一次還沒回來就不再發
  let inflight = null
  // 最後被點開的那封。點得快時先發的請求可能後到，
  // 不擋的話畫面上會跳出使用者已經不看的那封信。
  let wanted = null

  return {
    get mails() {
      return mails
    },
    set mails(v) {
      mails = v
    },
    get viewing() {
      return viewing
    },
    set viewing(v) {
      // 關掉檢視也要作廢在飛的請求，否則它一回來又把面板打開
      if (v === null) wanted = null
      viewing = v
    },

    load() {
      if (inflight) return inflight
      inflight = fetchList()
        .then((r) => { mails = r })
        .catch(fail)
        .finally(() => { inflight = null })
      return inflight
    },

    // 清單只有摘要，全文（body / html / links）點開時才拿一封
    async view(m) {
      wanted = m.id
      try {
        const full = await api.mail(m.id)
        if (wanted === m.id) viewing = full
      } catch (e) {
        if (wanted === m.id) fail(e)
      }
    },
  }
}
