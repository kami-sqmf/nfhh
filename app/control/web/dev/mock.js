// 在頁面載入前攔截 fetch，餵假資料。純瀏覽器端，伺服器完全不知情。
const now = Math.floor(Date.now() / 1000)
// 郵件假資料。全文（body / html / links）只從單封端點 /api/mail/{id} 出去，
// 清單只有摘要 —— mock 照同一個形狀，才擋得住「清單少了欄位」這種
// 在 dev 看起來好好的、上線才發現的錯。
const MAILS = [
  { id: 9, received_at: now - 60, sender: 'info@account.netflix.com',
    recipient: 'netflix@share.example.com', subject: '您的 Netflix 暫時存取碼',
    code: null, body: '我們收到下列裝置的暫時存取碼申請。', html: null,
    links: ['https://www.netflix.com/account/travel/verify?nftoken=abc'],
    primary_link: 'https://www.netflix.com/account/travel/verify?nftoken=abc',
    verified: true, platform: 'netflix', skip_reason: null },
  { id: 1, received_at: now - 180, sender: 'info@netflix.com', recipient: 'netflix@share.example.com', subject: '您的登入驗證碼',
    code: '3849', primary_link: null,
    body: 'Netflix\n\n您的登入驗證碼是 3849\n\n此驗證碼 15 分鐘內有效。',
    html: '<div style="font-family:sans-serif;padding:20px"><h2 style="color:#E50914">Netflix</h2>'
        + '<p>您的登入驗證碼是</p><p style="font-size:32px;font-weight:bold">3849</p>'
        + '<p style="color:#666;font-size:13px">此驗證碼 15 分鐘內有效。</p>'
        + '<img src="https://example.com/track.gif?u=victim" width="1" height="1">'
        + '<img src="https://example.com/logo.png" width="120" height="40" alt="遠端圖片（應被擋）">'
        + '<script>document.body.innerHTML="⚠️ SCRIPT 執行了 —— 沙箱失效"<\/script>'
        + '<p><a href="https://netflix.com/login">前往登入</a></p></div>',
    links: ['https://netflix.com/login'], verified: true, platform: 'netflix', skip_reason: null },
  { id: 2, received_at: now - 200, sender: 'no-reply@disneyplus.com', recipient: 'disney@share.example.com', subject: '一次性密碼',
    code: '207415', body: null, html: null, links: [], primary_link: null,
    verified: false, platform: 'disneyplus', skip_reason: null },
  { id: 3, received_at: now - 700000, sender: 'info@netflix.com', recipient: 'netflix@share.example.com', subject: '登入驗證碼',
    code: '5501', body: null, html: null, links: [], primary_link: null,
    verified: null, platform: 'netflix', skip_reason: null },
  // 沒碼、沒連結、沒過驗證：卡片的琥珀色措辭只有這種信才走得到（連結被後端扣下也長這樣）
  { id: 6, received_at: now - 900, sender: 'promo@netflix-rewards.example', recipient: 'netflix@share.example.com', subject: '您的存取碼在這裡',
    code: null, body: '請點連結取得存取碼。', html: null, links: [], primary_link: null,
    verified: false, platform: 'netflix', skip_reason: null },
]

// 命中排除字，因此不進任何人的驗證碼分頁 —— 只有管理收件匣看得到（設計 1n）
const INBOX_ONLY = [
  { id: 4, received_at: now - 90000, sender: 'info@netflix.com',
    recipient: 'netflix@share.example.com', subject: '本月精選片單',
    code: null, body: '本月新片⋯⋯', html: null, links: [], primary_link: null,
    verified: true, platform: 'netflix', skip_reason: '命中排除字 電子報' },
]

// 清單 DTO：後端的 MailSummary 就是少了這三個欄位
const summary = ({ body, html, links, ...rest }) => rest
const FULL_BY_ID = new Map([...MAILS, ...INBOX_ONLY].map((m) => [m.id, m]))

const S = {
  '/api/status': {
    logged_in: true, username: 'alex@example.com', my_ip: '198.51.100.7',
    my_ip_allowed: true, wan_ip: '203.0.113.10', lan_ip: null,
    max_per_user: 4, my_entry_count: 3, default_ttl_days: 7,
    platforms: [{ code: 'disneyplus', name: 'Disney+' }, { code: 'netflix', name: 'Netflix' }],
    my_platforms: ['disneyplus', 'netflix'],
    needs_bootstrap: false, passkey_count: 1, is_admin: true,
    dot_host: 'dns.example.com', dot_ready: true, mail_enabled: true,
    join_enabled: true, cf_enabled: true,
    entries: [
      { ip: '198.51.100.7', label: '咖啡廳', added_by: 'alex@example.com', added_at: now - 86400,
        expires_at: now + 6 * 86400 + 82800, ttl_days: 7, renewed_at: null,
        queries: { count: 42, last_at: now - 30 }, mine: true },
      { ip: '198.51.100.23', label: '家裡', added_by: 'alex@example.com', added_at: now - 200000,
        expires_at: now + 6 * 86400, ttl_days: 7, renewed_at: now - 3600,
        queries: { count: 7, last_at: now - 120 }, mine: true },
      { ip: '198.51.100.44', label: '公司', added_by: 'alex@example.com', added_at: now - 500000,
        expires_at: now + 39600, ttl_days: 7, renewed_at: null,
        queries: { count: 0, last_at: null }, mine: true },
    ],
  },
  '/api/mail': MAILS.map(summary),
  // 兩個人在同一秒做同一件事是真的會發生的（第 2、3 筆）——
  // 那正是稽核清單的 key 必須用 id 的理由
  '/api/audit': [
    { id: 412, at: now - 300, actor: 'alex@example.com', action: 'allow_add', detail: '加入 198.51.100.7，ttl 7d', client_ip: null },
    { id: 411, at: now - 480, actor: 'robin@example.com', action: 'login', detail: '以 passkey 登入', client_ip: null },
    { id: 410, at: now - 480, actor: 'sam@example.com', action: 'login', detail: '以 passkey 登入', client_ip: null },
    { id: 409, at: now - 3600, actor: null, action: 'allow_renewed', detail: '198.51.100.23 ttl=7d 仍有查詢活動', client_ip: null },
    { id: 408, at: now - 90000, actor: null, action: 'mail_sender_unverified', detail: 'dkim=pass header.d=amazonses.com · 觀察期', client_ip: null },
  ],
  '/api/members': [
    { id: 'u1', label: 'alex@example.com', role: 'admin', platforms: ['netflix', 'disneyplus'],
      passkey_count: 2, entries: [{ ip: '198.51.100.7', label: '咖啡廳', expires_at: now + 6 * 86400 }] },
    { id: 'u2', label: 'robin@example.com', role: 'admin', platforms: ['netflix'],
      passkey_count: 1, entries: [{ ip: '192.0.2.61', label: '媽媽的手機', expires_at: now + 3 * 86400 }] },
    { id: 'u3', label: 'sam@example.com', role: 'member', platforms: [], passkey_count: 1, entries: [] },
    { id: 'u4', label: 'jamie@example.com', role: 'member', platforms: ['disneyplus'],
      passkey_count: 1, entries: [{ ip: '192.0.2.88', label: '家裡', expires_at: now + 86400 }] },
  ],
  '/api/invite': [
    { email: 'casey@example.com', invited_by: 'robin@example.com', invited_at: now - 100000,
      revoked_at: null, used_at: null, used_by: null, platforms: ['netflix'] },
    { email: 'mei@example.com', invited_by: 'alex@example.com', invited_at: now - 20000,
      revoked_at: null, used_at: null, used_by: null, platforms: [] },
    { email: 'jamie@example.com', invited_by: 'alex@example.com', invited_at: now - 600000,
      revoked_at: null, used_at: now - 500000, used_by: 'u4', platforms: ['disneyplus'] },
  ],
  '/api/recipients': [
    { id: 1, mailbox: 'netflix@share.example.com', address: 'robin@example.com', label: null, enabled: true,
      added_by: null, added_at: now, cf_verified_at: now - 63072000, cf_checked_at: now },
    { id: 2, mailbox: 'netflix@share.example.com', address: 'alex@example.com', label: null, enabled: true,
      added_by: null, added_at: now, cf_verified_at: now - 86400, cf_checked_at: now },
    { id: 3, mailbox: 'disney@share.example.com', address: 'sam@example.com', label: null, enabled: false,
      added_by: null, added_at: now, cf_verified_at: null, cf_checked_at: now },
  ],
  '/api/passkeys': [
    { id: 'cred-1', nickname: 'iPhone 15', created_at: now - 5000000, last_used_at: now - 300 },
    { id: 'cred-2', nickname: 'MacBook Pro', created_at: now - 900000, last_used_at: now - 200000 },
    { id: 'cred-3', nickname: null, created_at: now - 200, last_used_at: null },
  ],
  '/api/settings': {
    sender_mode: 'observe', sender_domains: ['netflix.com', 'disneyplus.com'],
    code_keywords: ['驗證碼', 'verification code'], code_excludes: ['促銷', '電子報'],
    platform_senders: {
      netflix: ['netflix.com'],
      disneyplus: ['disneyplus@trx.mail2.disneyplus.com'],
    },
    unmatched_senders: [
      { address: 'no-reply@mail.hbomax.com', count: 3 },
      { address: 'account@primevideo.com', count: 1 },
    ],
  },
}
S['/api/mail/inbox'] = [...MAILS, ...INBOX_ONLY].map(summary)

const orig = window.fetch
window.fetch = async (url, opts) => {
  // 出口 IPv4 的探測（見 src/lib/ip.js）。dev 不該真的打外部服務 ——
  // 截圖要可重現，離線也要能跑。回的值跟 /api/status 的 my_ip 一致。
  if (/^https:\/\/(api\.ipify\.org|ipv4\.icanhazip\.com)\b/.test(String(url)))
    return new Response('198.51.100.7\n')

  const path = String(url).replace(/^https?:\/\/[^/]+/, '').split('?')[0]
  if (path.startsWith('/api/allow/') && path.endsWith('/queries'))
    return new Response(JSON.stringify([
      // 同網域的 A 與 AAAA 會在同一秒各記一筆 —— 真的長這樣，
      // 也是 seq 這個欄位存在的理由：這兩列從 at/domain 分不出來
      { seq: 812, at: now - 20, domain: 'ipv4-c001.nflxvideo.net' },
      { seq: 811, at: now - 20, domain: 'ipv4-c001.nflxvideo.net' },
      { seq: 807, at: now - 35, domain: 'api.netflix.com' },
      { seq: 803, at: now - 56, domain: 'occ-0-1.nflxso.net' },
      { seq: 790, at: now - 89, domain: 'www.netflix.com' },
    ]), { headers: { 'content-type': 'application/json' } })
  if (path in S)
    return new Response(JSON.stringify(S[path]), { headers: { 'content-type': 'application/json' } })
  // 清單只有摘要，全文走單封端點。查不到的 id 要跟後端一樣回錯誤 ——
  // 掉到下面那條 { ok: true } 的話，MailView 會拿一顆空殼當信件開起來。
  const one = path.match(/^\/api\/mail\/(\d+)$/)
  if (one && (opts?.method ?? 'GET') === 'GET') {
    const m = FULL_BY_ID.get(Number(one[1]))
    return new Response(JSON.stringify(m ?? { error: '查無此信件' }), {
      status: m ? 200 : 400,
      headers: { 'content-type': 'application/json' },
    })
  }
  if (path.startsWith('/api/'))
    return new Response(JSON.stringify({ ok: true }), { headers: { 'content-type': 'application/json' } })
  return orig(url, opts)
}
