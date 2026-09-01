// 面板的 HTTP 客戶端。
//
// 錯誤一律轉成 Error(message)，訊息直接來自後端 —— 後端的錯誤字串是
// 寫給使用者看的中文，不是給程式判斷用的代碼。這是刻意的：拒絕要說原因
// （1a 的原則之一），而原因只有後端知道。

// ⚠️ method 沒給時是「有 body 就 POST，沒有就 GET」。
// **不帶 body 的 POST 必須明寫 method**，否則會靜靜送成 GET 換回 405。
async function req(path, { method, body } = {}) {
  const res = await fetch(path, {
    method: method || (body ? 'POST' : 'GET'),
    headers: body ? { 'content-type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
    credentials: 'same-origin',
  })
  const data = await res.json().catch(() => ({}))
  if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`)
  return data
}

export const api = {
  // ip = 這個網路的公網 IPv4（見 lib/ip.js）。帶了它，my_ip / my_ip_allowed
  // 講的才是「這一戶」，而不是「開面板這台裝置連到 Cloudflare 用的位址」。
  status: (ip) => req(ip ? `/api/status?ip=${encodeURIComponent(ip)}` : '/api/status'),
  logout: () => req('/api/logout', { method: 'POST' }),

  // 加入流程
  joinStart: (email) => req('/api/join/start', { body: { email } }),
  joinVerify: (email, code) => req('/api/join/verify', { body: { email, code } }),
  // 邀請函連結：兌換成功等於信箱已驗證，下一步直接建 Passkey
  joinInvite: (token) => req('/api/join/invite', { body: { token } }),
  setMyEmail: (email) => req('/api/me/email', { body: { email } }),

  // 個人的 Passkey。一律只操作自己的 —— admin 也碰不到別人的憑證。
  passkeys: () => req('/api/passkeys'),
  renamePasskey: (id, label) => req(`/api/passkeys/${encodeURIComponent(id)}`, { body: { label } }),
  revokePasskey: (id) => req(`/api/passkeys/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  // 推送通知。訂閱以裝置為單位，一律只操作自己的。
  pushKey: () => req('/api/push/key'),
  pushSubs: () => req('/api/push/subs'),
  pushSubscribe: (body) => req('/api/push/subs', { body }),
  pushUnsubscribe: (id) => req(`/api/push/subs/${id}`, { method: 'DELETE' }),
  // 這台裝置自己退訂。endpoint 不外流到清單裡，所以退訂走這條而非 id。
  pushUnsubscribeSelf: (endpoint) => req('/api/push/unsubscribe', { body: { endpoint } }),
  pushCheck: (endpoint) => req('/api/push/check', { body: { endpoint } }),
  notifyPrefs: () => req('/api/me/notify'),
  setNotifyPrefs: (codes, expiry) => req('/api/me/notify', { body: { codes, expiry } }),

  // 自己的轉發設定。admin 那頁管的是全部人、以 mailbox 為單位；
  // 這裡是一顆總開關，切掉名下所有 mailbox。
  myForwarding: () => req('/api/me/forwarding'),
  setMyForwarding: (enabled) => req('/api/me/forwarding', { body: { enabled } }),
  resendForwardingVerify: () => req('/api/me/forwarding/resend', { method: 'POST' }),

  // 幫某筆收件人在 Cloudflare 建位址／重寄驗證信（管理員）
  verifyRecipient: (id) => req(`/api/recipients/${id}/verify`, { method: 'POST' }),

  // 平台 → 收件信箱的對應。刻意讓 admin 明說，不用「代號@網域」推 ——
  // 那個約定對 Disney+ 是錯的（代號 disneyplus、信箱 disney@）。
  setMailbox: (platform, mailbox) => req('/api/mailboxes', { body: { platform, mailbox } }),
  purgeMailbox: (mailbox) =>
    req(`/api/mailboxes/${encodeURIComponent(mailbox)}`, { method: 'DELETE' }),

  // 白名單
  allow: (body) => req('/api/allow', { body }),
  unallow: (ip) => req(`/api/allow/${encodeURIComponent(ip)}`, { method: 'DELETE' }),
  rename: (ip, label) => req(`/api/allow/${encodeURIComponent(ip)}`, { body: { label } }),
  queries: (ip) => req(`/api/allow/${encodeURIComponent(ip)}/queries`),

  // 驗證碼
  mails: () => req('/api/mail'),
  inbox: () => req('/api/mail/inbox'),
  deleteMail: (id) => req(`/api/mail/${id}`, { method: 'DELETE' }),
  purgeMails: () => req('/api/mail', { method: 'DELETE' }),

  // 管理
  audit: () => req('/api/audit'),
  settings: () => req('/api/settings'),
  saveSettings: (body) => req('/api/settings', { method: 'PUT', body }),
  members: () => req('/api/members'),
  setRole: (id, role) => req(`/api/members/${id}/role`, { body: { role } }),
  removeMember: (id) => req(`/api/members/${id}`, { method: 'DELETE' }),
  grant: (id, platform) => req(`/api/members/${id}/platforms`, { body: { platform } }),
  revoke: (id, platform) =>
    req(`/api/members/${id}/platforms/${encodeURIComponent(platform)}`, { method: 'DELETE' }),
  recipients: () => req('/api/recipients'),
  addRecipient: (mailbox, address, label) =>
    req('/api/recipients', { body: { mailbox, address, label: label || null } }),
  toggleRecipient: (id, enabled) =>
    req(`/api/recipients/${id}/enabled`, { body: { enabled } }),
  removeRecipient: (id) => req(`/api/recipients/${id}`, { method: 'DELETE' }),
  invites: () => req('/api/invite'),
  invite: (email, platforms = []) => req('/api/invite', { body: { email, platforms } }),
  uninvite: (email) => req(`/api/invite/${encodeURIComponent(email)}`, { method: 'DELETE' }),
}

// ── WebAuthn ────────────────────────────────────────
// 規格的二進位欄位在 JSON 裡是 base64url，而瀏覽器 API 要 ArrayBuffer。
// 這兩支從舊面板原樣搬過來 —— 已經驗證過的東西沒有理由重寫。

const b2a = (s) => {
  s = s.replace(/-/g, '+').replace(/_/g, '/')
  const bin = atob(s + '='.repeat((4 - (s.length % 4)) % 4))
  const u = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i)
  return u.buffer
}

const a2b = (buf) => {
  let s = ''
  for (const b of new Uint8Array(buf)) s += String.fromCharCode(b)
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '')
}

/** 註冊一把 passkey。三種用途共用：建第一個帳號、家人加入、加註備援裝置。 */
export async function registerPasskey({ email, bootstrapToken, nickname } = {}) {
  const o = await req('/api/register/start', {
    body: {
      email: email ?? null,
      bootstrap_token: bootstrapToken ?? null,
      nickname: nickname ?? guessDeviceName(),
    },
  })
  const pk = o.publicKey
  pk.challenge = b2a(pk.challenge)
  pk.user.id = b2a(pk.user.id)
  ;(pk.excludeCredentials || []).forEach((c) => (c.id = b2a(c.id)))

  const c = await navigator.credentials.create({ publicKey: pk })
  await req('/api/register/finish', {
    body: {
      id: c.id,
      rawId: a2b(c.rawId),
      type: c.type,
      response: {
        attestationObject: a2b(c.response.attestationObject),
        clientDataJSON: a2b(c.response.clientDataJSON),
      },
      clientExtensionResults: c.getClientExtensionResults(),
    },
  })
}

export async function loginPasskey(email) {
  const o = await req('/api/login/start', { body: { email } })
  const pk = o.publicKey
  pk.challenge = b2a(pk.challenge)
  ;(pk.allowCredentials || []).forEach((c) => (c.id = b2a(c.id)))

  const c = await navigator.credentials.get({ publicKey: pk })
  await req('/api/login/finish', {
    body: {
      id: c.id,
      rawId: a2b(c.rawId),
      type: c.type,
      response: {
        authenticatorData: a2b(c.response.authenticatorData),
        clientDataJSON: a2b(c.response.clientDataJSON),
        signature: a2b(c.response.signature),
        userHandle: c.response.userHandle ? a2b(c.response.userHandle) : null,
      },
      clientExtensionResults: c.getClientExtensionResults(),
    },
  })
}

/**
 * 可探索登入：不告訴伺服器我是誰，由裝置自己挑一把 passkey。
 *
 * `mediation` 決定 UI 形態：
 *   - 'optional'（預設）→ 跳出系統的憑證選擇器。設計 1j 的按鈕走這條。
 *   - 'conditional'     → 不跳窗，把選項掛進輸入框的自動填入。
 *
 * 兩者共用同一個後端挑戰（`allowCredentials` 是空的），差別只在瀏覽器
 * 怎麼問使用者。
 */
export async function loginDiscoverable({ conditional = false, signal } = {}) {
  // 可探索登入不需要送任何東西，但端點是 POST（它會在 session 建立挑戰狀態），
  // 所以 method 要明寫 —— 不寫的話 req() 會推論成 GET。
  const o = await req('/api/login/any/start', { method: 'POST' })
  const pk = o.publicKey
  pk.challenge = b2a(pk.challenge)
  ;(pk.allowCredentials || []).forEach((c) => (c.id = b2a(c.id)))

  const c = await navigator.credentials.get({
    publicKey: pk,
    ...(conditional ? { mediation: 'conditional' } : {}),
    ...(signal ? { signal } : {}),
  })
  if (!c) throw new Error('沒有可用的 Passkey')

  return await req('/api/login/any/finish', {
    body: {
      id: c.id,
      rawId: a2b(c.rawId),
      type: c.type,
      response: {
        authenticatorData: a2b(c.response.authenticatorData),
        clientDataJSON: a2b(c.response.clientDataJSON),
        signature: a2b(c.response.signature),
        userHandle: c.response.userHandle ? a2b(c.response.userHandle) : null,
      },
      clientExtensionResults: c.getClientExtensionResults(),
    },
  })
}

/** 這個瀏覽器支援把 passkey 掛進自動填入嗎。 */
export const supportsConditionalUi = () =>
  typeof PublicKeyCredential !== 'undefined' &&
  PublicKeyCredential.isConditionalMediationAvailable?.().catch(() => false)

/**
 * 猜一個裝置名當預設值。使用者可以改，但多數人不會 —— 而「iPhone」比
 * 一片空白好認太多，尤其是三個月後回來看「哪一把是我弄丟的那台」。
 *
 * userAgent 不可靠也無所謂：猜錯只是預設名字不好聽，改一下就好。
 */
export function guessDeviceName() {
  const ua = navigator.userAgent
  if (/iPhone/.test(ua)) return 'iPhone'
  if (/iPad/.test(ua)) return 'iPad'
  if (/Android/.test(ua)) return 'Android 裝置'
  if (/Macintosh/.test(ua)) return 'Mac'
  if (/Windows/.test(ua)) return 'Windows 電腦'
  return '這台裝置'
}

/**
 * WebAuthn 的錯誤名稱很少，訊息卻是瀏覽器自己的英文原文。沒對應到的話
 * 使用者會看到一句他讀不懂、也不知道該怎麼辦的話。
 *
 * 每一條都要能回答「那我現在該做什麼」，而不只是描述發生了什麼。
 */
export function passkeyError(e) {
  switch (e.name) {
    case 'NotAllowedError':
      // 取消、逾時、找不到可用憑證都是這個 —— 規格刻意不區分，
      // 避免網站探測「這台裝置上有沒有某個帳號」。
      return '已取消，或這台裝置上沒有可用的 Passkey'
    case 'InvalidStateError':
      return '這台裝置已經註冊過了'
    case 'NotSupportedError':
      return '這個瀏覽器不支援免輸入帳號的登入，請改用下面的 Email 登入'
    case 'SecurityError':
      return '網域設定不符，Passkey 無法在這個位址使用'
    case 'AbortError':
      return '已中斷'
    default:
      // 連 name 都對不上時至少講清楚下一步，別只丟英文原文
      return `Passkey 無法使用（${e.name || '未知錯誤'}），請改用下面的 Email 登入`
  }
}
