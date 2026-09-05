<script>
  import { app, notify, fail } from '../lib/state.svelte.js'
  import { api } from '../lib/api.js'
  import { mailList } from '../lib/mail.svelte.js'
  import CodeCard from '../components/CodeCard.svelte'
  import MailView from '../components/MailView.svelte'

  // 設計 1e。這裡只有「我有權限的平台」且「抽得到驗證碼」的信 ——
  // 兩層過濾都在後端做，前端拿到什麼就顯示什麼。
  const list = mailList(api.mails)
  const mails = $derived(list.mails)

  const platform = (code) => app.status?.platforms?.find((p) => p.code === code)
  const platformName = (code) => platform(code)?.name ?? code

  async function del(id) {
    try {
      await api.deleteMail(id)
      list.mails = mails.filter((m) => m.id !== id)
    } catch (e) { fail(e) }
  }

  $effect(() => { list.load() })
  $effect(() => {
    const t = setInterval(() => { if (!document.hidden) list.load() }, 20000)
    return () => clearInterval(t)
  })
</script>

<header class="px-5 pt-5 pb-4 bg-surface">
  <h1 class="text-head font-bold">驗證碼</h1>
  <p class="mt-1.5 text-body text-fg-faint">保留 14 天 · 逾期自動清除</p>
</header>

<div class="px-5 pt-3.5 flex flex-col gap-2.5">
  {#each mails as m (m.id)}
    <CodeCard mail={m} platformName={platformName(m.platform)} platformColor={platform(m.platform)?.color} ondelete={del}
              onview={list.view} />
  {:else}
    <div class="bg-surface rounded-lg p-5 text-body leading-relaxed text-fg-muted text-pretty">
      {#if !app.status?.my_platforms?.length}
        你還沒有被授權任何平台，所以看不到任何驗證碼。
        請管理員到「成員管理」把平台開給你。
      {:else}
        目前沒有驗證碼。平台寄出驗證信後會自動出現在這裡。
      {/if}
    </div>
  {/each}
</div>

<MailView mail={list.viewing} onclose={() => (list.viewing = null)} />
