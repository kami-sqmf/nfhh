<script>
  import { app, refresh } from './lib/state.svelte.js'
  import Msg from './components/Msg.svelte'
  import BottomNav from './components/BottomNav.svelte'

  import Bootstrap from './screens/Bootstrap.svelte'
  import Login from './screens/Login.svelte'
  import Join from './screens/Join.svelte'
  import JoinCode from './screens/JoinCode.svelte'
  import JoinInvite from './screens/JoinInvite.svelte'
  import Home from './screens/Home.svelte'
  import Allow from './screens/Allow.svelte'
  import Codes from './screens/Codes.svelte'
  import Guide from './screens/Guide.svelte'
  import Admin from './screens/Admin.svelte'
  import Inbox from './screens/admin/Inbox.svelte'
  import Members from './screens/admin/Members.svelte'
  import Recipients from './screens/admin/Recipients.svelte'
  import Sender from './screens/admin/Sender.svelte'

  const tabs = { home: Home, allow: Allow, codes: Codes, guide: Guide, admin: Admin }
  const subs = { inbox: Inbox, members: Members, recipients: Recipients, sender: Sender }
  // Email 驗證碼登入沿用加入流程的兩個畫面，只換 mode（文案與 API 不同，版型相同）
  const auth = {
    login: [Login],
    join: [Join],
    joincode: [JoinCode],
    invited: [JoinInvite],
    loginemail: [Join, 'login'],
    logincode: [JoinCode, 'login'],
  }

  // 驗證碼登入的 session 是弱認證，後端的 admin 端點一律拒絕 ——
  // 子頁進去只會一直載入失敗閃紅，所以直接停在管理首頁讓它解釋原因。
  const weakAuth = $derived(app.status?.auth_via === 'otp')
  const Screen = $derived(
    app.tab === 'admin' && app.sub && !weakAuth ? subs[app.sub] : tabs[app.tab] ?? Home
  )
  const [Auth, authMode] = $derived(auth[app.authStep] ?? auth.login)

  $effect(() => { refresh() })
</script>

{#if !app.status}
  <div class="grid place-items-center min-h-[100dvh] text-body text-fg-faint">載入中…</div>
{:else if app.status.needs_bootstrap}
  <Bootstrap />
{:else if !app.status.logged_in}
  <Msg />
  <Auth mode={authMode} />
{:else}
  <Msg />
  <Screen />
  <BottomNav />
{/if}
