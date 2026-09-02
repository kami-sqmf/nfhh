/**
 * Cloudflare Email Worker — 推信進面板，再依面板的答覆轉發給家人。
 *
 * 每封信先 POST /api/mail/ingest，面板解析 MIME、跑完寄件者驗證與驗證碼
 * 篩選器後回一份 forward_to，這支才照著轉。判斷放面板是因為篩選器的關鍵字
 * 要比對內文，而這支不解析 MIME；轉發留在這裡是因為 message.forward()
 * 只有 Worker 有。
 *
 * ⚠️ 面板掛掉不能讓信轉不出去 —— 推送失敗就退回 FORWARD_MAP 照送，
 *    且刻意不做任何過濾。少轉幾封廣告 vs 漏掉一組碼，代價不對等。
 *    **拒收**（4xx）不算掛掉，只轉給 FALLBACK_TO。
 *
 * 無 import、無相依，直接貼進 Dashboard 的 Worker 編輯器。
 *
 * ── 環境變數 ──────────────────────────────────────────
 *   PANEL_ENDPOINT  https://dnf.example.com/api/mail/ingest   一般變數
 *   PANEL_SECRET    同面板的 NFHH_MAIL_SECRET              Secret
 *   FALLBACK_TO     你一個人的信箱                          一般變數
 *   FORWARD_MAP     {"netflix@share.example.com":["a@x.com"]}  Secret（家人個資）
 *
 *   ⚠️ FORWARD_MAP 是面板停機時唯一的退路，要跟面板「轉發收件人」頁同步。
 *      平常讀不到，所以過期了也沒有徵兆，直到停機那天。
 *
 *   FALLBACK_TO 永遠會被加進名單，連面板說「不用轉」時也是 —— 篩選器設錯
 *   才看得見。部署步驟見 docs/SETUP.md §6。
 */

// 面板正常是幾十毫秒，5 秒是留給大信的上傳與解析。使用者正在等驗證碼。
const INGEST_TIMEOUT_MS = 5000;

export default {
  async email(message, env) {
    const outcome = await pushToPanel(message, env);
    if (outcome.kind === "ok") {
      const panel = outcome.panel;
      console.log(
        `面板回覆：篩選器${panel.actionable ? "通過" : "擋下"}` +
          ` verified=${panel.verified} 家人 ${(panel.forward_to || []).length} 人` +
          `${panel.new === false ? "（重送，已存在）" : ""}`
      );
    }
    const targets = decideTargets(outcome, message.to, env);

    if (targets.length === 0) {
      console.log(`⚠️ ${mask(message.to)} 沒有任何轉發目標，信件不會轉出去`);
    }

    // 並行而非逐一 await：序列時每封信 12~35 秒。
    const results = await Promise.allSettled(
      targets.map((addr) => message.forward(addr))
    );
    results.forEach((r, i) => {
      // 只記遮罩後的位址，避免全址留在 Cloudflare 日誌
      if (r.status === "fulfilled") {
        console.log(`轉址至 ${mask(targets[i])} 成功`);
      } else {
        console.log(`轉址至 ${mask(targets[i])} 失敗，已跳過：`, r.reason && r.reason.message);
      }
    });
  },

  // 只靠 Email Routing 觸發。留 fetch() 是讓掃描器的請求不要變成 500。
  // ⚠️ 不要在這裡讀 env —— PANEL_SECRET 就在裡面。
  async fetch() {
    return new Response(null, { status: 404 });
  },
};

/**
 * 推信給面板並分類結果，絕不 throw：
 *   { kind: "ok", panel }            2xx 且 JSON 合法
 *   { kind: "rejected", status }     任何 4xx、或 2xx 但 JSON 壞掉 —— 永久性
 *   { kind: "unavailable", reason }  5xx、逾時、連不上 —— 暫時性
 *   { kind: "unconfigured" }         沒有 PANEL_ENDPOINT / PANEL_SECRET —— 部署狀態，
 *                                    不是攻擊面（攻擊者改不了 Worker 的環境變數）。
 *                                    面板還沒上線時 FORWARD_MAP 是唯一的轉發路徑。
 *
 * x-nfhh-mailbox 帶的是信封收件位址：只靠 To: 表頭在轉寄鏈上會查錯名單。
 */
async function pushToPanel(message, env) {
  if (!env.PANEL_ENDPOINT || !env.PANEL_SECRET) {
    console.log("⚠️ PANEL_ENDPOINT / PANEL_SECRET 未設定：沒有寄件者驗證，只依 FORWARD_MAP 轉發");
    return { kind: "unconfigured" };
  }

  try {
    const res = await fetch(env.PANEL_ENDPOINT, {
      method: "POST",
      headers: {
        authorization: `Bearer ${env.PANEL_SECRET}`,
        "content-type": "message/rfc822",
        "x-nfhh-mailbox": String(message.to || ""),
      },
      body: await new Response(message.raw).arrayBuffer(),
      signal: AbortSignal.timeout(INGEST_TIMEOUT_MS),
    });
    return await classifyResponse(res);
  } catch (e) {
    console.log("push failed:", e && e.message);
    return { kind: "unavailable", reason: e && e.message };
  }
}

/** 把面板的回應分成「可用」「拒收」「不可用」三類。 */
export async function classifyResponse(res) {
  if (res.status >= 500) return { kind: "unavailable", reason: `HTTP ${res.status}` };
  if (!res.ok) {
    // 面板的錯誤回應是 {"error": …}，不含信件內容，可安全記錄
    const detail = await res.text().catch(() => "");
    console.log("panel rejected:", res.status, detail.slice(0, 200));
    return { kind: "rejected", status: res.status };
  }
  try {
    return { kind: "ok", panel: await res.json() };
  } catch {
    return { kind: "rejected", status: res.status, reason: "bad json" };
  }
}

/**
 * 決定轉發名單。只有「面板不可用」才准走 FORWARD_MAP；「面板拒收」只給
 * FALLBACK_TO —— 拒收的信本來就不該無過濾地送進家人信箱。
 */
export function decideTargets(outcome, to, env) {
  switch (outcome.kind) {
    case "ok":
      return withFallback(outcome.panel.forward_to, env);
    case "rejected":
      console.log(`⚠️ 面板拒收（${outcome.status}），只轉給 FALLBACK_TO，不走 FORWARD_MAP`);
      return withFallback([], env);
    case "unconfigured":
      return withFallback(fallbackRecipients(to, env), env);
    default:
      console.log("⚠️ 面板無回應，改用 FORWARD_MAP 轉發（面板不會有這封信的紀錄）");
      return withFallback(fallbackRecipients(to, env), env);
  }
}

/** 面板不可用／未設定時查 FORWARD_MAP。未知位址回空陣列，呼叫端仍會補 FALLBACK_TO。 */
function fallbackRecipients(to, env) {
  if (!env.FORWARD_MAP) {
    console.log("⚠️ FORWARD_MAP 未設定，本次只轉給 FALLBACK_TO");
    return [];
  }
  try {
    const extra = JSON.parse(env.FORWARD_MAP)[String(to || "").toLowerCase()];
    if (Array.isArray(extra)) return extra;
    console.log(`${mask(to)} 不在 FORWARD_MAP 中，只轉給 FALLBACK_TO`);
  } catch (e) {
    console.log("⚠️ FORWARD_MAP 不是合法 JSON：", e && e.message);
  }
  return [];
}

/** FALLBACK_TO 排第一，接上名單並去重、濾空。 */
function withFallback(addrs, env) {
  const out = [];
  for (const addr of [env.FALLBACK_TO, ...(addrs || [])]) {
    if (typeof addr === "string" && addr && !out.includes(addr)) out.push(addr);
  }
  return out;
}

/** 日誌用遮罩：al***@example.com */
function mask(addr) {
  const s = String(addr || "");
  const at = s.lastIndexOf("@");
  if (at < 1) return "***";
  return `${s.slice(0, Math.min(2, at))}***${s.slice(at)}`;
}
