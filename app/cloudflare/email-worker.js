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
    const panel = await pushToPanel(message, env);

    let targets;
    if (panel) {
      console.log(
        `面板回覆：篩選器${panel.actionable ? "通過" : "擋下"}` +
          ` verified=${panel.verified} 家人 ${(panel.forward_to || []).length} 人` +
          `${panel.new === false ? "（重送，已存在）" : ""}`
      );
      targets = withFallback(panel.forward_to, env);
    } else {
      // 「面板掛了」的唯一信號。密鑰打錯也走這裡 —— 面板的錯誤一律回 400，
      // 這支分不出「密鑰不符」和「面板忙」。
      console.log("⚠️ 面板無回應，改用 FORWARD_MAP 轉發（面板不會有這封信的紀錄）");
      targets = withFallback(fallbackRecipients(message.to, env), env);
    }

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
 * 推信給面板並回傳它的判定；逾時、連不上、非 2xx、JSON 壞掉一律回 null
 * 讓呼叫端走退路，絕不 throw。
 *
 * x-nfhh-mailbox 帶的是信封收件位址：只靠 To: 表頭在轉寄鏈上會查錯名單。
 */
async function pushToPanel(message, env) {
  if (!env.PANEL_ENDPOINT || !env.PANEL_SECRET) {
    console.log("PANEL_ENDPOINT / PANEL_SECRET 未設定，略過推送");
    return null;
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

    if (!res.ok) {
      // 面板的錯誤回應是 {"error": …}，不含信件內容，可安全記錄
      const detail = await res.text().catch(() => "");
      console.log("panel rejected:", res.status, detail.slice(0, 200));
      return null;
    }
    return await res.json();
  } catch (e) {
    console.log("push failed:", e && e.message);
    return null;
  }
}

/** 面板無回應時查 FORWARD_MAP。未知位址回空陣列，呼叫端仍會補 FALLBACK_TO。 */
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
