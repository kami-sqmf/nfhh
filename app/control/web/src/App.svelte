<script>
  import { app, refresh } from './lib/state.svelte.js'
  import Msg from './components/Msg.svelte'
  import BottomNav from './components/BottomNav.svelte'

  import Bootstrap from './screens/Bootstrap.svelte'
  import NeedEmail from './screens/NeedEmail.svelte'
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
  const auth = { login: Login, join: Join, joincode: JoinCode, invited: JoinInvite }

  const Screen = $derived(
    app.tab === 'admin' && app.sub ? subs[app.sub] : tabs[app.tab] ?? Home
  )
  const Auth = $derived(auth[app.authStep] ?? Login)

  $effect(() => { refresh() })
</script>

{#if !app.status}
  <div class="grid place-items-center min-h-[100dvh] text-body text-fg-faint">載入中…</div>
{:else if app.status.needs_bootstrap}
  <Bootstrap />
{:else if !app.status.logged_in}
  <Msg />
  <Auth />
{:else if app.status.needs_email}
  <Msg />
  <NeedEmail />
{:else}
  <Msg />
  <Screen />
  <BottomNav />
{/if}
