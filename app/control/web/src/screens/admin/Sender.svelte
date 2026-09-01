<script>
  import { notify, fail } from '../../lib/state.svelte.js'
  import { api } from '../../lib/api.js'
  import SubHeader from '../../components/SubHeader.svelte'
  import TagEditor from '../../components/TagEditor.svelte'
  import Toggle from '../../components/Toggle.svelte'
  import PlatformMark from '../../components/PlatformMark.svelte'
  import { app } from '../../lib/state.svelte.js'

  // 設計 1m。三檔在 v6 之後管的是**顯示**，不是轉發 ——
  // 轉發在 Cloudflare Worker 手上，面板改不到。畫面上要講清楚。
  let cfg = $state(null)
  let busy = $state(false)

  const platforms = $derived(app.status?.platforms ?? [])
  const unmatched = $derived(cfg?.unmatched_senders ?? [])

  // 這個位址已經被指派給某個平台了嗎 —— 用來把建議清單裡已處理的標掉
  const assigned = (addr) =>
    Object.values(cfg?.platform_senders ?? {}).some((v) => v.includes(addr))

  function addSender(code, addr) {
    const map = { ...(cfg.platform_senders ?? {}) }
    map[code] = [...(map[code] ?? []), addr]
    save({ platform_senders: map })
  }

  const modes = [
    { id: 'off', label: '關閉', desc: '不驗證寄件者，所有信件一律顯示。' },
    { id: 'observe', label: '觀察期', desc: '未通過驗證的信仍會顯示在驗證碼分頁，標成琥珀色並寫進稽核。' },
    { id: 'enforce', label: '強制', desc: '未通過驗證的信不進驗證碼分頁，只留在這裡的收件匣。' },
  ]

  const current = $derived(modes.find((m) => m.id === cfg?.sender_mode))

  async function save(patch) {
    const next = { ...cfg, ...patch }
    cfg = next
    busy = true
    try {
      // unmatched_senders 是伺服器算給我們看的，不是設定的一部分，
      // 送回去只會被當成未知欄位
      const { unmatched_senders, ...body } = next
      const r = await api.saveSettings(body)
      notify(
        r.reclassified > 0 ? `已儲存 · ${r.reclassified} 封信有了平台歸屬` : '已儲存',
        true
      )
      // 重判之後「認不出的寄件者」會變少，重讀一次才看得到
      cfg = await api.settings()
    } catch (e) { fail(e) } finally { busy = false }
  }

  $effect(() => {
    api.settings().then((r) => (cfg = r)).catch(fail)
  })
</script>

<SubHeader title="寄件者驗證" sub="在此修改，存檔即生效" />

{#if cfg}
  <div class="px-5 pt-3.5 flex flex-col gap-2.5">
    <div class="bg-surface rounded-lg p-4">
      <h2 class="text-body font-semibold">顯示策略</h2>
      <div class="mt-2.5 flex gap-1.5">
        {#each modes as m (m.id)}
          <button
            onclick={() => save({ sender_mode: m.id })}
            disabled={busy}
            class="flex-1 py-3 min-h-0 rounded-sm text-body
                   {cfg.sender_mode === m.id
                     ? (m.id === 'observe' ? 'bg-watch text-white font-semibold'
                        : m.id === 'enforce' ? 'bg-fg text-canvas font-semibold'
                        : 'bg-line-firm text-fg font-semibold')
                     : 'border-[1.5px] border-line-firm text-fg-muted'}"
          >{m.label}</button>
        {/each}
      </div>
      <p class="mt-2.5 text-body leading-relaxed text-fg-muted text-pretty">{current?.desc}</p>

      <div class="mt-3 pt-3 border-t border-line">
        <div class="flex items-center justify-between gap-3">
          <div class="min-w-0">
            <div class="text-body font-semibold">未通過驗證的信也轉發</div>
            <p class="mt-1 text-label leading-relaxed text-fg-muted text-pretty">
              關掉 = 只轉發通過寄件者驗證的信。這是<b class="font-semibold">轉發</b>策略，
              跟上面的顯示策略是兩件事。
            </p>
          </div>
          <Toggle
            checked={!cfg.forward_enforce}
            disabled={busy}
            label="未通過驗證的信也轉發"
            onchange={(v) => save({ forward_enforce: !v })}
          />
        </div>
      </div>
    </div>

    <div class="bg-surface rounded-lg p-4">
      <h2 class="text-body font-semibold">白名單網域</h2>
      <div class="mt-2.5">
        <TagEditor
          tags={cfg.sender_domains}
          placeholder="網域"
          onchange={(v) => save({ sender_domains: v })}
        />
      </div>
      <p class="mt-2.5 text-label leading-relaxed text-fg-muted text-pretty">
        DKIM 的 <span class="font-mono">header.d</span> 或 DMARC 必須對上其中一個網域才算通過。
        比對的是<b class="font-semibold">簽章網域</b>，不是信封寄件者 —— 後者誰都能偽造。
      </p>
    </div>

    <!-- 平台寄件者對應。收件信箱的 local part 本來就能推出平台，但那要求
         每個平台各有一個信箱；用同一個 catch-all 收全部時那個線索就沒了。 -->
    <div class="bg-surface rounded-lg p-4">
      <h2 class="text-body font-semibold">平台寄件者位址</h2>
      <p class="mt-1 text-label leading-relaxed text-fg-muted text-pretty">
        判定一封信屬於哪個平台時，<b class="font-semibold">寄件者優先於收件信箱</b> ——
        位址對應是你明確設定的，信箱只是路由意圖。
      </p>

      {#each platforms as p (p.code)}
        <div class="mt-3.5 pt-3.5 border-t border-line first:border-0 first:pt-0">
          <div class="flex items-center gap-2 mb-2">
            <PlatformMark code={p.code} name={p.name} color={p.color} size="sm" />
            <span class="text-body font-medium">{p.name}</span>
          </div>
          <TagEditor
            tags={cfg.platform_senders?.[p.code] ?? []}
            tone="ok"
            placeholder="位址或網域"
            onchange={(v) => save({ platform_senders: { ...cfg.platform_senders, [p.code]: v } })}
          />
        </div>
      {:else}
        <p class="mt-3 text-label text-fg-faint">沒有啟用中的平台。</p>
      {/each}

      <p class="mt-3.5 text-label leading-relaxed text-fg-faint text-pretty">
        含 <span class="font-mono">@</span> 比對完整位址；只給網域則比對網域<b class="font-semibold">含子網域</b>
        （<span class="font-mono">netflix.com</span> 會命中
        <span class="font-mono">info@members.netflix.com</span>）。
        網域比對認 <span class="font-mono">.</span> 邊界，
        <span class="font-mono">evil-netflix.com</span> 不會誤中。
      </p>

      {#if unmatched.length}
        <div class="mt-3.5 pt-3.5 border-t border-line">
          <div class="text-body font-semibold">認不出平台的寄件者</div>
          <p class="mt-1 text-label leading-relaxed text-fg-muted text-pretty">
            這些信已經收到了，但判不出屬於哪個平台，所以不會出現在任何人的驗證碼分頁。
            點一下指派給某個平台。
          </p>
          <div class="mt-2.5 flex flex-col gap-2">
            {#each unmatched as u (u.address)}
              <!-- 位址一行、指派按鈕一行。擠在同一行時位址會被截成
                   no-reply@mail.hbo… 這種認不出來的片段，而平台一多就爆版。 -->
              <div class="px-3 py-2.5 rounded-sm bg-canvas">
                <div class="font-mono text-label break-all">{u.address}</div>
                <div class="mt-1.5 flex items-center gap-2 flex-wrap">
                  <span class="text-meta text-fg-faint shrink-0">
                    {u.count} 封 · 指派給
                  </span>
                  {#each platforms as p (p.code)}
                    <button
                      onclick={() => addSender(p.code, u.address)}
                      disabled={busy || assigned(u.address)}
                      class="flex items-center gap-1.5 px-2.5 py-1.5 min-h-0 rounded-pill
                             text-label font-medium border-[1.5px] border-line-firm
                             disabled:opacity-35"
                    >
                      <PlatformMark code={p.code} name={p.name} color={p.color} size="sm" />
                      {p.name}
                    </button>
                  {/each}
                </div>
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </div>

    <div class="bg-surface rounded-lg p-4">
      <div class="flex items-baseline justify-between">
        <h2 class="text-body font-semibold">驗證碼篩選器</h2>
        <span class="text-label text-fg-faint">同時決定轉發與顯示</span>
      </div>

      <div class="mt-3 flex items-start gap-3">
        <span class="w-11 shrink-0 text-label text-fg-faint pt-2">關鍵字</span>
        <TagEditor tags={cfg.code_keywords} tone="ok" placeholder="關鍵字"
                   onchange={(v) => save({ code_keywords: v })} />
      </div>
      <div class="mt-2.5 flex items-start gap-3">
        <span class="w-11 shrink-0 text-label text-fg-faint pt-2">排除字</span>
        <TagEditor tags={cfg.code_excludes} tone="bad" placeholder="排除字"
                   onchange={(v) => save({ code_excludes: v })} />
      </div>

      <p class="mt-3 text-label leading-relaxed text-fg-faint text-pretty">
        <b class="font-semibold">抽到驗證碼、或命中任一關鍵字</b>才算數；命中排除字一律不算。
        不算數的信不會轉給家人，也不會出現在驗證碼分頁 —— 轉發與顯示用的是同一套判準。
      </p>
      <div class="mt-2.5 pt-2.5 border-t border-line text-label leading-relaxed text-fg-faint text-pretty">
        排除字<b class="font-semibold">只比對主旨</b>，關鍵字才比對主旨與內文。
        這個不對稱是刻意的：排除是排他條件，命中一次就永遠看不到，而正常的
        驗證碼信內文常會順帶提到那些字（例如暫時存取碼信會解釋「同戶裝置」規則）。
      </div>
    </div>
  </div>
{/if}
