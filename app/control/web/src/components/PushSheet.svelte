<script>
  import { notify } from '../lib/state.svelte.js'
  import { api, guessDeviceName } from '../lib/api.js'
  import { pushState, enablePush } from '../lib/push.js'
  import Sheet from './Sheet.svelte'
  import Toggle from './Toggle.svelte'
  import IconBell from '~icons/lucide/bell'
  import IconAlert from '~icons/lucide/triangle-alert'

  // 設計 3a / 3b。同一個彈層，靠環境決定顯示哪一張。
  //
  // 出現時機是這台裝置第一次進面板時（見 Home.svelte 的 maybeAskPush）——
  // 設計稿原本是「複製驗證碼之後」，但那時這一輪的碼已經抄完了，
  // 通知要在下一組碼來之前設好才有意義。
  let { open = false, onclose } = $props()

  let state = $state('ready')
  // 先問要不要開通知，答應了才講「iPhone 得先加到主畫面」——
  // 一進來就丟安裝步驟，等於在他還沒決定要不要之前先派功課
  let step = $state('ask')
  // 「還是從分頁開的」要講在彈層裡：那條 toast 出現在最上面，
  // 會被彈層自己的遮罩壓住，等於沒講
  let stuck = $state(false)
  let busy = $state(false)
  // 預設只開「新驗證碼」，跟後端的欄位預設值一致
  let codes = $state(true)
  let expiry = $state(false)

  $effect(() => {
    if (open) {
      step = 'ask'
      stuck = false
      pushState().then((s) => (state = s))
    }
  })

  async function enable() {
    // iOS 分頁連 PushManager 都沒有，問不了權限 —— 這時「開啟通知」的
    // 下一步是安裝說明，不是權限對話框
    if (state === 'homescreen') {
      step = 'howto'
      stuck = false
      return
    }
    busy = true
    try {
      // 必須由這次點擊直接呼叫 —— iOS 要求權限請求來自明確手勢
      const ok = await enablePush(guessDeviceName())
      if (!ok) {
        notify('你選了不允許。要開啟的話得到瀏覽器的網站設定裡改回來。')
        state = await pushState()
        return
      }
      await api.setNotifyPrefs(codes, expiry)
      notify('已開啟通知', true)
      onclose?.()
    } catch (e) {
      notify(e.message || String(e))
    } finally {
      busy = false
    }
  }

  // 他多半還在 Safari 分頁裡按這顆，而從分頁按不會有任何變化 —— 要講清楚
  async function recheck() {
    state = await pushState()
    if (state === 'homescreen') {
      stuck = true
      return
    }
    step = 'ask' // 推得動了，回到那顆「開啟通知」讓他按
  }
</script>

<Sheet {open} {onclose} title="">
  {#if step === 'howto'}
    <!-- 3b：他說要開通知了，才講 iOS Safari 得先加到主畫面 -->
    <div class="flex items-start gap-3">
      <div class="shrink-0 w-11 h-11 rounded-md bg-watch-bg grid place-items-center">
        <IconBell width="20" height="20" class="text-watch-fg" />
      </div>
      <div>
        <div class="text-title font-semibold leading-snug text-pretty">
          iPhone 要先把面板加到主畫面
        </div>
        <p class="mt-1.5 text-item leading-relaxed text-fg-muted text-pretty">
          Safari 只允許已加入主畫面的網站發通知。加完之後從主畫面的圖示打開面板，
          再回來開啟通知就可以了。
        </p>
      </div>
    </div>

    <div class="mt-5 p-4 rounded-md bg-canvas flex flex-col gap-3">
      <!-- 第 3 步是踩過才知道的：關掉「開啟為網頁 App」會加成一般書籤，
           推送**靜默失敗** —— 不報錯、不跳權限。iOS 26 起才預設開啟。 -->
      {#each [
        '點 Safari 下方的<b class="font-semibold">分享</b>圖示',
        '往下找到<b class="font-semibold">加入主畫面</b>',
        '確認<b class="font-semibold">開啟為網頁 App</b>是開著的，再按新增',
        '從主畫面開啟面板，回到這裡按<b class="font-semibold">開啟通知</b>',
      ] as line, i (i)}
        <div class="flex gap-3 items-start">
          <span
            class="shrink-0 w-[22px] h-[22px] rounded-full bg-fg text-canvas
                   font-mono text-micro font-semibold grid place-items-center"
          >{i + 1}</span>
          <span class="text-item leading-relaxed text-fg-strong">{@html line}</span>
        </div>
      {/each}
    </div>

    {#if stuck}
      <!-- 按了「我已經加到主畫面了」但還是在分頁裡。這是最容易卡住的一步，
           講在按鈕正上方，而且要看起來像出事了 -->
      <div class="mt-3 p-3.5 rounded-sm bg-bad-bg flex items-start gap-2.5">
        <IconAlert width="18" height="18" class="shrink-0 mt-0.5 text-bad-fg" />
        <div class="text-bad-fg">
          <div class="text-item font-semibold leading-snug text-pretty">還是從 Safari 分頁開的</div>
          <p class="mt-1 text-label leading-relaxed text-pretty">
            加到主畫面之後還要再開一次：關掉這個分頁，改從主畫面上新出現的圖示打開面板，
            再回到這裡開啟通知。
          </p>
        </div>
      </div>
    {:else}
      <p class="mt-3 p-3 rounded-sm bg-watch-bg text-label leading-relaxed text-watch-fg text-pretty">
        Android Chrome 不用這一步，直接按開啟通知即可。
      </p>
    {/if}

    <button
      onclick={recheck}
      class="mt-4 w-full py-4 rounded-md bg-fg text-canvas text-lead font-semibold"
    >我已經加到主畫面了</button>
  {:else if state === 'blocked'}
    <!-- 拒絕過就只能自己去改，面板再問一百次也不會跳 -->
    <div class="flex items-start gap-3">
      <div class="shrink-0 w-11 h-11 rounded-md bg-bad-bg grid place-items-center">
        <IconBell width="20" height="20" class="text-bad-fg" />
      </div>
      <div>
        <div class="text-title font-semibold leading-snug text-pretty">通知被這個瀏覽器擋住了</div>
        <p class="mt-1.5 text-item leading-relaxed text-fg-muted text-pretty">
          之前選過「不允許」，面板沒有辦法再問一次。到瀏覽器的網站設定裡把這個網站的
          通知改成允許，再回來開啟。
        </p>
      </div>
    </div>
  {:else if state === 'unsupported'}
    <div class="flex items-start gap-3">
      <div class="shrink-0 w-11 h-11 rounded-md bg-canvas grid place-items-center">
        <IconBell width="20" height="20" class="text-fg-faint" />
      </div>
      <div>
        <div class="text-title font-semibold leading-snug text-pretty">這個瀏覽器不支援通知</div>
        <p class="mt-1.5 text-item leading-relaxed text-fg-muted text-pretty">
          換 Android 的 Chrome 或 iPhone 的 Safari 就可以。驗證碼照樣會出現在面板上。
        </p>
      </div>
    </div>
  {:else}
    <!-- 3a：推得動 -->
    <div class="flex items-start gap-3">
      <div class="shrink-0 w-11 h-11 rounded-md bg-ok-bg grid place-items-center">
        <IconBell width="20" height="20" class="text-ok-fg" />
      </div>
      <div>
        <div class="text-title font-semibold leading-snug text-pretty">
          新驗證碼一到，直接通知你
        </div>
        <p class="mt-1.5 text-item leading-relaxed text-fg-muted text-pretty">
          開啟後，OTT 寄來新的登入驗證碼時，這支手機會跳出通知並帶上那組碼。
          不用一直回來重新整理面板。
        </p>
      </div>
    </div>

    <div class="mt-5 p-4 rounded-md bg-canvas flex flex-col gap-3">
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="text-item font-semibold">新驗證碼</div>
          <div class="mt-0.5 text-label text-fg-faint">Netflix、Disney+ 等寄來登入碼時</div>
        </div>
        <Toggle checked={codes} label="新驗證碼通知" onchange={(v) => (codes = v)} />
      </div>
      <div class="h-px bg-line"></div>
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="text-item font-semibold">授權快到期</div>
          <div class="mt-0.5 text-label text-fg-faint">白名單的 IP 剩 24 小時時提醒</div>
        </div>
        <Toggle checked={expiry} label="授權到期提醒" onchange={(v) => (expiry = v)} />
      </div>
    </div>

    <p class="mt-3 text-label leading-relaxed text-fg-faint text-pretty">
      {#if state === 'homescreen'}
        iPhone 還要先把面板加到主畫面才收得到通知，按下去會告訴你怎麼加。
      {:else}
        按下「開啟通知」後，瀏覽器會再問一次權限 —— 要選「允許」才會生效。
      {/if}
      之後隨時可以在右上角的<b class="font-semibold">個人設定</b>關掉。
    </p>

    <button
      onclick={enable}
      disabled={busy}
      class="mt-4 w-full py-4 rounded-md bg-ok text-white text-lead font-semibold
             disabled:opacity-50"
    >{busy ? '…' : '開啟通知'}</button>
  {/if}

  <button
    onclick={onclose}
    class="mt-2 w-full py-3.5 rounded-md text-item font-semibold text-fg-muted"
  >之後再說</button>
</Sheet>
