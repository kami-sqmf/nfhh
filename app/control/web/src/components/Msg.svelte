<script>
  import { app } from '../lib/state.svelte.js'
</script>

{#if app.msg}
  <!-- fixed 而不是 sticky：sticky 仍然佔版面高度，訊息一出現就把整頁撐高
       —— 登入頁是 min-h-[100dvh]，於是多出一條捲軸、畫面跟著跳一下。
       置中的寫法跟 BottomNav 同一組：body 的 max-width 管不到固定定位。

       z-index 要大於 Sheet 的 z-50：彈層裡也會 notify，壓在遮罩下面等於沒講。

       整條都可以點掉，不是只有那個 ✕：脫離文件流之後它會蓋住頁首，
       而錯誤訊息刻意不自動退場，得留一個夠大的目標讓人把它移開。 -->
  <button
    type="button"
    onclick={() => (app.msg = null)}
    aria-live="polite"
    class="fixed top-0 inset-x-0 z-[60] mx-auto max-w-[480px] min-h-0
           flex items-center justify-between gap-3 px-5 py-2.5 text-body text-left
           {app.msg.ok ? 'bg-ok-bg text-ok-fg' : 'bg-bad-bg text-bad-fg'}"
  >
    <span>{app.msg.text}</span>
    <!-- 只是關閉的提示，名稱由訊息本身提供，不必念出來 -->
    <span aria-hidden="true" class="font-mono opacity-45">✕</span>
  </button>
{/if}
