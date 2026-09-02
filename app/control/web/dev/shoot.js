// 用 CDP 驅動無頭 Chrome：模擬深色、點擊、截圖、收集 console 錯誤。
// Chrome 的 --force-dark-mode 不會翻轉 prefers-color-scheme，只能走 CDP。
const [, , url, plan] = process.argv
const steps = JSON.parse(plan)

const targets = await (await fetch('http://127.0.0.1:9333/json/list')).json()
const page = targets.find((t) => t.type === 'page')
const ws = new WebSocket(page.webSocketDebuggerUrl)
await new Promise((r) => (ws.onopen = r))

let id = 0
const pending = new Map()
const errors = []
ws.onmessage = (e) => {
  const m = JSON.parse(e.data)
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id) }
  if (m.method === 'Runtime.exceptionThrown')
    errors.push('例外: ' + (m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text))
  if (m.method === 'Runtime.consoleAPICalled' && ['error', 'warning'].includes(m.params.type))
    errors.push(m.params.type + ': ' + m.params.args.map((a) => a.value ?? a.description).join(' '))
}
const send = (method, params = {}) =>
  new Promise((res) => { pending.set(++id, res); ws.send(JSON.stringify({ id, method, params })) })

// 在任何頁面腳本之前注入，這樣 app 的第一次 fetch 就已經被攔截
const mock = process.env.MOCK && (await Bun.file(process.env.MOCK).text())
if (mock) await send('Page.addScriptToEvaluateOnNewDocument', { source: mock })

await send('Runtime.enable')
await send('Page.enable')
await send('Emulation.setDeviceMetricsOverride',
  { width: 390, height: 844, deviceScaleFactor: 2, mobile: true })

for (const s of steps) {
  if (s.dark !== undefined) {
    await send('Emulation.setEmulatedMedia', {
      media: 'screen',
      features: [{ name: 'prefers-color-scheme', value: s.dark ? 'dark' : 'light' }],
    })
  }
  if (s.goto) {
    await send('Page.navigate', { url })
    await new Promise((r) => setTimeout(r, 2500))
  }
  if (s.click) {
    const r = await send('Runtime.evaluate', {
      expression: `(() => { const els=[...document.querySelectorAll('button,a')];
        const el=els.find(e=>e.textContent.trim().includes(${JSON.stringify(s.click)}));
        if(!el) return '找不到: ' + ${JSON.stringify(s.click)}; el.click(); return 'ok'; })()`,
      returnByValue: true,
    })
    if (r.result.value !== 'ok') console.log('⚠️', r.result.value)
    await new Promise((r) => setTimeout(r, s.wait ?? 900))
  }
  if (s.type) {
    await send('Runtime.evaluate', {
      expression: `(() => { const i=document.querySelector('input'); if(!i) return;
        const set=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;
        set.call(i, ${JSON.stringify(s.type)});
        i.dispatchEvent(new Event('input',{bubbles:true})); })()`,
    })
    await new Promise((r) => setTimeout(r, 400))
  }
  if (s.shot) {
    const { data } = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true })
    await Bun.write(`${process.env.OUT ?? '/tmp/shots'}/${s.shot}.png`, Buffer.from(data, 'base64'))
    console.log('📸', s.shot)
  }
}
console.log(errors.length ? '\n⚠️ Console：\n' + [...new Set(errors)].join('\n') : '\n✅ 無 console 錯誤')
ws.close()
