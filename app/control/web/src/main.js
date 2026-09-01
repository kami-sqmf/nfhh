import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'
import { notify } from './lib/state.svelte.js'

// 開機就註冊而不是等到要訂閱時 —— iOS 要求權限請求來自明確手勢，
// 那一刻不能再插一段 await register() 進去。
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').catch(() => {
    // 註冊失敗只代表沒有通知，面板其他功能照常
  })

  // Android 通知上那顆「複製」按鈕（剪貼簿在 worker 裡碰不到，頁面代勞）
  navigator.serviceWorker.addEventListener('message', async (e) => {
    if (e.data?.type !== 'copy-code' || !e.data.code) return
    try {
      await navigator.clipboard.writeText(e.data.code)
      notify(`已複製 ${e.data.code}`, true)
    } catch {
      // 剛被喚醒時不一定拿得到剪貼簿權限。講清楚下一步，別靜靜地沒反應。
      notify('請點下方卡片上的驗證碼複製')
    }
  })
}

export default mount(App, { target: document.getElementById('app') })
