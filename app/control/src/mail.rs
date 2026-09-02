//! 信件解析與驗證碼抽取。Worker 只負責搬運原始信件，解析全在這邊做。

use serde::Serialize;

/// 表頭欄位的上限。RFC 5322 一行是 998 bytes，主旨與位址實務上遠小於此；
/// 超過的不是正常信，是想撐大資料庫的人。
pub const MAX_SUBJECT_CHARS: usize = 500;
pub const MAX_ADDR_CHARS: usize = 320;
pub const MAX_MSGID_CHARS: usize = 998;

/// 單一 URL 的上限。再長的不是要按的連結，是要塞進資料庫的酬載。
pub const MAX_LINK_LEN: usize = 2048;

/// 主旨純粹給人看，截斷不會讓任何判斷讀出錯誤結論。
fn cap(s: String, max: usize) -> String {
    if s.chars().count() <= max { s } else { s.chars().take(max).collect() }
}

/// 位址與 Message-ID 不能截斷，只能整個丟掉。
///
/// 截斷會**憑空造出一個看起來合理的值**：`evil@netflix.com.attacker.example`
/// 剛好切在第 320 個字就成了 `evil@netflix.com`；而 Message-ID 是去重鍵，
/// 截短等於讓寄件者自由製造碰撞，用一封信蓋掉另一封。
/// 超長的本來就不是正常信，寧可當它沒有這個表頭。
///
/// 代價講明白：拒收超長的 Message-ID 等於那封信沒有去重鍵（SQLite 的
/// UNIQUE 容許多個 NULL），Worker 重送幾次就進幾筆。比讓寄件者拿碰撞
/// 蓋掉別人的信好 —— 而超長本身已經被表頭上限攔在正常信之外。
fn reject_over(s: String, max: usize) -> Option<String> {
    (s.chars().count() <= max).then_some(s)
}

#[derive(Debug, Serialize, Default)]
pub struct Parsed {
    pub message_id: Option<String>,
    pub sender: Option<String>,
    pub recipient: Option<String>,
    pub subject: Option<String>,
    pub date: Option<i64>,
    pub body: String,
    /// 原始 HTML，供面板在沙箱 iframe 內呈現。
    pub html: Option<String>,
    pub code: Option<String>,
    pub links: Vec<String>,
    /// 寄件者驗證結果。決定這封信要不要真的扇出給家人。
    pub auth: SenderAuth,
}

/// 從 `Authentication-Results` 表頭抽出的驗證結果。
/// 信任錨點是通過驗證的品牌網域（DKIM `header.d`），不是信封寄件者網域。
#[derive(Debug, Serialize, Default, Clone)]
pub struct SenderAuth {
    /// 所有 `dkim=pass` 的 `header.d`，任一命中白名單即可。
    pub dkim_domains: Vec<String>,
    /// `dmarc=pass` 時的 `header.from`，作為第二條比對路徑。
    pub dmarc_from: Option<String>,
    /// 只記錄不參與判斷，用來對照日誌。
    pub envelope_domain: Option<String>,
}

impl SenderAuth {
    pub fn is_trusted(&self, allowed: &[String]) -> bool {
        let hit = |d: &String| {
            allowed
                .iter()
                .any(|x| d == x || d.ends_with(&format!(".{x}")))
        };
        self.dkim_domains.iter().any(hit) || self.dmarc_from.as_ref().is_some_and(hit)
    }

    /// 給日誌用的一行摘要。不含收件人，可安全記錄。
    pub fn summary(&self) -> String {
        format!(
            "dkim.d=[{}] dmarc.from={} envelope={}",
            self.dkim_domains.join(","),
            self.dmarc_from.as_deref().unwrap_or("無"),
            self.envelope_domain.as_deref().unwrap_or("無")
        )
    }
}

/// RFC 8601：第一個分號前是 authserv-id，可帶版本號，如 `mx.cloudflare.net 1`。
fn authserv_matches(value: &str, expected: &str) -> bool {
    let head = value.split(';').next().unwrap_or("");
    head.split_whitespace()
        .next()
        .is_some_and(|id| id.eq_ignore_ascii_case(expected))
}

/// 把表頭正規化成「只剩結構」的樣子，之後才好照 `;` 與空白切開。做兩件事：
///
/// 1. 拿掉 CFWS 註解。`dkim=pass (1024-bit key)` 裡的括號段落是給人看的，
///    內容不受限制 —— 留著它等於留一條夾帶判決的路。註解在語法上等於一個
///    空白，所以換成空白而不是直接刪掉：`dkim=(x)pass` 變成 `dkim= pass`，
///    兩個 token 都不成立，正好落在安全的一邊。括號沒閉合時後面整段丟掉，
///    同樣往安全的一邊倒。
/// 2. 吃掉引號字串裡的結構字元（`;` 與空白類）。`;`、空白、`=` 都是 RFC 5321
///    引號本地部的合法字元，所以 `smtp.mailfrom="; dkim=pass header.d=x"@evil`
///    這種**合法**的信封位址，會在切段之後長出一段假的判決。反過來，RFC 8601
///    的 `reason=` 本來就是引號字串、裡面本來就可能有 `;`，把它當段落分隔會
///    誤殺真信。兩邊的解法是同一個：引號內不留任何能開段落或開 token 的字元。
///
/// 引號內的**內容**保留（不是抹掉），`header.d="netflix.com"` 才照樣讀得出來。
/// `\` 跳脫的下一個字元一律換成 `_`，`\"` 因此不會提早結束字串。
///
/// 這個轉換是冪等的，重複跑不會改變結果。
fn strip_cfws(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let (mut depth, mut quoted, mut esc) = (0usize, false, false);
    for c in raw.chars() {
        if esc {
            esc = false;
            if depth == 0 {
                out.push('_');
            }
            continue;
        }
        match c {
            '\\' if quoted => esc = true,
            '"' if depth == 0 => {
                quoted = !quoted;
                out.push('"');
            }
            '(' if !quoted => {
                if depth == 0 {
                    out.push(' ');
                }
                depth += 1;
            }
            ')' if !quoted => depth = depth.saturating_sub(1),
            ';' | ' ' | '\t' | '\r' | '\n' if quoted && depth == 0 => {}
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// 這個 token 是不是 `method=pass`。RFC 8601 的形狀是
/// `method [ "/" version ] "=" result`，版本號可有可無。
fn is_pass(token: &str, method: &str) -> bool {
    let Some((m, result)) = token.split_once('=') else {
        return false;
    };
    m.split('/').next() == Some(method) && result == "pass"
}

/// 解析 `Authentication-Results`。各方法以 `;` 分隔，先切段再於段內比對。
///
/// 段內一律**以 token 為單位**比對，不做子字串搜尋：判決必須是該段的第一個
/// token，`header.d=` 這類屬性也必須自成一個 token。理由是 MTA 會把寄件者
/// 可控的字串原樣抄進自己的表頭 —— `smtp.mailfrom=` 就是信封寄件者 ——
/// 於是 `dkim=pass.header.d=netflix.com@evil.com` 這個合法信箱名，在子字串
/// 比對下會被讀成一則「通過」的判決。
fn parse_auth_results(raw: &str) -> (Vec<String>, Option<String>) {
    let lower = strip_cfws(raw).to_lowercase();
    let mut dkim = Vec::new();
    let mut dmarc = None;
    for seg in lower.split(';') {
        // 判決是每段的開頭，之後才是 ptype.property。段首以外的都是資料。
        let Some(verdict) = seg.split_whitespace().next() else {
            continue;
        };
        if is_pass(verdict, "dkim")
            && let Some(d) = property_domain(seg, "header.d=")
            && !dkim.contains(&d)
        {
            dkim.push(d);
        }
        if is_pass(verdict, "dmarc") && dmarc.is_none() {
            dmarc = property_domain(seg, "header.from=");
        }
    }
    (dkim, dmarc)
}

/// 取出 `key` 這個屬性的網域值。`key` 必須是整個 token 的開頭 ——
/// 別的屬性的**值**裡出現同名字串不算數。
fn property_domain(seg: &str, key: &str) -> Option<String> {
    seg.split_whitespace()
        .find_map(|t| t.strip_prefix(key))
        .and_then(domain_value)
}

/// 只認網域字元，遇到別的就停（`@` 之後是本機部分，不是網域）。
fn domain_value(raw: &str) -> Option<String> {
    let v: String = raw
        .chars()
        .skip_while(|c| *c == '"' || *c == '\'')
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
        .collect();
    // 結尾的點是合法的 FQDN 寫法，但白名單比對不帶它
    let v = v.trim_end_matches('.').to_string();
    (!v.is_empty()).then_some(v)
}

pub fn parse(raw: &[u8], authserv_id: &str) -> Parsed {
    let Some(msg) = mail_parser::MessageParser::default().parse(raw) else {
        // 解析失敗仍保留原始內容，驗證碼可能還在裡面
        let body = String::from_utf8_lossy(raw).chars().take(20_000).collect::<String>();
        let code = extract_code(&body);
        return Parsed { body, code, ..Default::default() };
    };

    // 只採信**第一個**、且 authserv-id 是我們自己收信端的 Authentication-Results。
    // 寄件者可以在原始信裡塞任意同名表頭，但收信端的表頭永遠加在最頂端；
    // 把全部串起來看，等於讓寄件者替自己蓋「已認證」的章。
    //
    // 先挑出第一個同名表頭、再讀它的值：兩步不能合成一步。合起來寫的話，
    // 第一個表頭的值是空的（`as_text()` 給 None）就會靜靜地滑到第二個，
    // 而那個第二個正是寄件者塞的。
    //
    // 挑段之前先正規化：authserv-id 前面可以合法地帶一段 CFWS 註解
    // （`(added by cf) mx.cloudflare.net; …`），不先拿掉會把真信判成別人寫的。
    let auth = msg
        .headers()
        .iter()
        .find(|h| h.name().eq_ignore_ascii_case("authentication-results"))
        .and_then(|h| h.value().as_text())
        .map(strip_cfws)
        .filter(|v| authserv_matches(v, authserv_id))
        .unwrap_or_default();
    let (dkim_domains, dmarc_from) = parse_auth_results(&auth);

    let subject = msg.subject().map(|s| s.to_string());

    // text/plain 與 HTML 兩段都要取：純文字段常是空殼，內容只在 HTML 裡
    let text = msg.body_text(0).map(|t| t.to_string()).unwrap_or_default();
    let raw_html = msg.body_html(0).map(|h| h.to_string());
    let html = raw_html.as_deref().map(html_to_text).unwrap_or_default();

    // 顯示用取比較有料的那份
    let body = if html.chars().count() > text.chars().count() { html.clone() } else { text.clone() };
    // 先截斷再抽連結：以前是對完整 body 抽，body 的 20k 上限對 links 沒有效果。
    // 代價：只出現在第 20 000 字之後的連結會抽不到 —— 那種信的行動按鈕
    // （primary_link）就沒了，使用者只剩內文可讀。正常的驗證信遠短於此，
    // 拿它換掉「一封信塞爆 links 欄位」的路。
    let body: String = body.chars().take(20_000).collect();
    let links = extract_links(&body);

    // 搜尋範圍涵蓋主旨與兩段內文
    let haystack = format!(
        "{}\n{}\n{}",
        subject.as_deref().unwrap_or(""),
        text,
        html
    );

    let sender = msg
        .from()
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .and_then(|s| reject_over(s.to_string(), MAX_ADDR_CHARS));

    // envelope_domain 只記錄不參與判斷，跟著被拒收的位址一起消失 ——
    // 位址我們都不採信了，沒道理還把它的網域寫進日誌。
    let envelope_domain = sender
        .as_deref()
        .and_then(|s| s.split('@').nth(1))
        .map(|d| d.to_lowercase());

    Parsed {
        auth: SenderAuth { dkim_domains, dmarc_from, envelope_domain },
        message_id: msg
            .message_id()
            .and_then(|s| reject_over(s.to_string(), MAX_MSGID_CHARS)),
        sender,
        recipient: msg
            .to()
            .and_then(|a| a.first())
            .and_then(|a| a.address())
            .and_then(|s| reject_over(s.to_string(), MAX_ADDR_CHARS)),
        subject: subject.map(|s| cap(s, MAX_SUBJECT_CHARS)),
        date: msg.date().map(|d| d.to_timestamp()),
        code: extract_code(&haystack),
        links,
        body,
        // 上限避免夾帶大量內嵌圖片的信把資料庫撐爆
        html: raw_html.map(|h| h.chars().take(400_000).collect()),
    }
}

/// 出現在驗證碼附近的字眼。距離越近，該數字越可能是我們要的。
const HINTS: &[&str] = &[
    "code", "passcode", "otp", "verification", "verify", "one-time", "temporary",
    "驗證碼", "驗證", "代碼", "密碼", "一次性", "臨時", "认证", "验证码",
];

/// 數字後面接這些就是數量／日期，不是驗證碼。
const UNITS: &[&str] = &["年", "月", "日", "%", "元", "px", "分", "秒", "小時"];

/// 從文字中找出最像驗證碼的數字。沒有提示字眼就回傳 None，不猜。
pub fn extract_code(text: &str) -> Option<String> {
    let lower = text.to_lowercase();

    // 提示字眼的位置。一個都沒有就不猜。
    let mut hints: Vec<usize> = Vec::new();
    for h in HINTS {
        let mut from = 0;
        while let Some(p) = lower[from..].find(h) {
            hints.push(from + p);
            from += p + h.len();
        }
    }
    if hints.is_empty() {
        return None;
    }

    let bytes = lower.as_bytes();
    let mut best: Option<(usize, String)> = None;
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let (end, len) = (i, i - start);
        if !(4..=8).contains(&len) {
            continue;
        }
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        if end < bytes.len() && bytes[end].is_ascii_alphanumeric() {
            continue;
        }
        let digits = &lower[start..end];

        // ── 必須是獨立的一個詞：所在片段去掉標點後要正好等於這串數字 ──
        // ── 不能是更長識別碼的一部分：看前後緊鄰的字元 ──
        // 連接符出現在數字旁代表它鑲在 UUID / HTML 實體 / 路徑裡。
        // 用緊鄰字元而非空白切詞：中文「您的驗證碼：123456」整句沒有空格。
        const BAD_PREV: &[char] = &['-', '_', '&', '#', '=', '/', '+', '.', '%'];
        const BAD_NEXT: &[char] = &['-', '_', '&', ';', '=', '/', '+', '%'];
        if lower[..start].chars().next_back().is_some_and(|c| BAD_PREV.contains(&c)) {
            continue;
        }
        if lower[end..].chars().next().is_some_and(|c| BAD_NEXT.contains(&c)) {
            continue;
        }

        // ── 後面緊跟單位／日期字 → 是數量不是碼 ──
        let after = lower[end..].trim_start();
        if UNITS.iter().any(|u| after.starts_with(u)) {
            continue;
        }
        // ── 前面是版權符號 ──
        let before = lower[..start].trim_end();
        if before.ends_with('©') || before.ends_with("(c)") {
            continue;
        }

        let dist = hints
            .iter()
            .map(|h| h.abs_diff(start))
            .min()
            .unwrap_or(usize::MAX);

        // 四位數又落在年份範圍，提示字眼的距離門檻加嚴
        let limit = if len == 4
            && digits.parse::<u32>().map_or(false, |v| (1900..=2100).contains(&v))
        {
            40
        } else {
            120
        };
        if dist > limit {
            continue;
        }
        if best.as_ref().map_or(true, |(d, _)| dist < *d) {
            best = Some((dist, digits.to_string()));
        }
    }
    best.map(|(_, d)| d)
}

/// 抽出信中的連結。前端會把完整網址原樣顯示，不用錨點文字。
pub fn extract_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(p) = rest.find("https://") {
        let tail = &rest[p..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '"' || c == '<' || c == '>' || c == ')')
            .unwrap_or(tail.len());
        let url: String = tail[..end].trim_end_matches(['.', ',', ';']).to_string();
        if url.len() > 12 && url.len() <= MAX_LINK_LEN && !out.contains(&url) {
            out.push(url);
        }
        rest = &tail[end.max(1)..];
        if out.len() >= 10 {
            break;
        }
    }
    out
}

/// 解 HTML 實體，含數字型的 `&#8199;` / `&#x2007;`。
/// 數字型必須解碼，否則實體裡的數字會變成假的驗證碼候選。
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        // 實體最長十來個字元。上限算「字元」不算 byte，才不會切在多位元字元中間
        let window_end = tail.char_indices().nth(12).map(|(i, _)| i).unwrap_or(tail.len());
        let Some(semi) = tail[..window_end].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let ch = if let Some(hex) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = ent.strip_prefix('#') {
            dec.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            match ent {
                "nbsp" => Some(' '),
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "shy" | "zwnj" | "zwj" => Some('\u{200b}'),
                _ => None,
            }
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    // 清掉不可見的排版填充字元，避免製造假的詞邊界
    out.chars()
        .filter(|c| !matches!(c, '\u{200b}'..='\u{200f}' | '\u{2007}' | '\u{feff}' | '\u{034f}'))
        .collect()
}

/// 極簡 HTML 轉純文字。目的只是讓人讀得懂並讓驗證碼浮出來，不求還原排版。
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut chars = html.char_indices().peekable();
    // 只做 ASCII 小寫：byte 長度與字元邊界跟原字串完全相同，下面用原字串的
    // `i` 去切 `lower` 才安全。要比對的標籤名本來就是 ASCII。
    let lower = html.to_ascii_lowercase();

    while let Some((i, c)) = chars.next() {
        if c == '<' {
            // script / style 的內容整段丟掉。跳過之後直接 continue 外層迴圈 ——
            // 不然會落到下面「吃到下一個 `>` 為止」那段，把區塊之後的正常內容
            // 一起當成標籤內容吞掉。
            let mut skipped = false;
            for tag in ["script", "style"] {
                if lower[i..].starts_with(&format!("<{tag}")) {
                    if let Some(e) = lower[i..].find(&format!("</{tag}>")) {
                        let skip_to = i + e + tag.len() + 3;
                        while let Some((j, _)) = chars.peek() {
                            if *j >= skip_to { break; }
                            chars.next();
                        }
                        skipped = true;
                    }
                }
            }
            if skipped {
                continue;
            }
            // 區塊標籤換行
            if lower[i..].starts_with("<br") || lower[i..].starts_with("<p")
                || lower[i..].starts_with("<div") || lower[i..].starts_with("<tr")
                || lower[i..].starts_with("</p") || lower[i..].starts_with("</div")
            {
                out.push('\n');
            }
            for (_, c2) in chars.by_ref() {
                if c2 == '>' { break; }
            }
        } else {
            out.push(c);
        }
    }

    let out = decode_entities(&out);

    // 壓掉空白：連續空行縮成一行，行首行尾去空白
    let mut lines: Vec<&str> = Vec::new();
    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() {
            if lines.last().map_or(false, |l: &&str| l.is_empty()) { continue; }
            lines.push("");
        } else {
            lines.push(t);
        }
    }
    lines.join("\n").trim().to_string()
}

/// 這封信有沒有「可以拿去用的東西」——— 決定它進不進驗證碼分頁。
///
/// 抽得到碼當然算。抽不到碼但命中關鍵字也算：Netflix 的「暫時存取碼」
/// 就是這種 —— 碼不在信裡，在信中「取得存取碼」那顆按鈕後面。那封信正是
/// 這個專案存在的理由，不能因為抽不到數字就對家人隱藏。
///
/// ⚠️ **排除字只比對主旨，關鍵字才比對主旨＋內文。** 這個不對稱是刻意的，
/// 而且是踩過才知道的：
///
/// 排除字的用途是「整類信不要」，而那個類別是**主旨**在宣告的
/// （電子報、促銷、通知）。內文裡的順帶提及不構成排除 —— 實際案例是
/// 排除字設了「同戶」，而暫時存取碼信的內文正好在解釋規則時寫著
/// 「此代碼僅限⋯⋯在 Netflix 同戶裝置以外的裝置暫時使用」，
/// 於是最該顯示的那封信被自己的說明文字擋掉了。
///
/// 關鍵字比對內文則沒有這個風險：那是**包含**條件，多命中一封只是多顯示
/// 一封；排除是**排他**條件，多命中一封就是永遠看不到。
///
/// 附帶好處：Worker 不解析 MIME、本來就只看得到主旨，這樣兩邊的排除判斷
/// 天生一致，不必靠巧合對齊。
pub fn is_actionable(
    subject: Option<&str>,
    body: Option<&str>,
    has_code: bool,
    keywords: &[String],
    excludes: &[String],
) -> bool {
    let hits = |pats: &[String], hay: &str| {
        let hay = hay.to_lowercase();
        pats.iter()
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .any(|p| hay.contains(&p))
    };

    let subject = subject.unwrap_or_default();
    if hits(excludes, subject) {
        return false;
    }
    has_code || hits(keywords, &format!("{subject} {}", body.unwrap_or_default()))
}

/// 信件頁尾的樣板連結。這些每封行銷信都有，不是使用者要按的東西。
///
/// 比對的是路徑片段而非完整網址 —— 各平台的頁尾長得不一樣，
/// 但「說明 / 條款 / 隱私 / 退訂」這幾類是通用的。
const BOILERPLATE: &[&str] = &[
    "/help", "/support", "/contactus", "/contact", "/legal",
    "termsofuse", "/terms", "privacypolicy", "/privacy",
    "unsubscribe", "optout", "/preferences", "/browse",
];

/// 信裡「要按的那個連結」。
///
/// 取第一個非樣板連結：信件的行動呼籲一定排在頁尾之前，而頁尾佔了連結
/// 數量的大半（實測一封暫時存取碼信有 10 個連結，其中 8 個是頁尾）。
pub fn primary_link(links: &[String]) -> Option<String> {
    links
        .iter()
        .find(|u| {
            // 上限在讀取端也擋一次：MAX_LINK_LEN 是後加的，比它早進庫的列
            // 還帶著超長連結，而這顆值是清單回應裡唯一帶連結的欄位。
            // 長度先判，才不會為了比對樣板去 lowercase 一條 8 MiB 的字串。
            if u.len() > MAX_LINK_LEN {
                return false;
            }
            let low = u.to_lowercase();
            !BOILERPLATE.iter().any(|b| low.contains(b))
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_digits_near_hint() {
        assert_eq!(extract_code("Your verification code is 4821."), Some("4821".into()));
        assert_eq!(extract_code("您的驗證碼：123456"), Some("123456".into()));
    }

    #[test]
    fn ignores_digits_with_no_hint() {
        // 沒有提示字眼就不猜
        assert_eq!(extract_code("Copyright 2026 Netflix, 1234 Main St"), None);
    }

    #[test]
    fn ignores_embedded_digits() {
        // width="1234" 之類不該被當成驗證碼
        assert_eq!(extract_code("code: <img width=600px> abc1234"), None);
    }

    fn kw() -> Vec<String> {
        ["驗證碼", "存取碼", "verification code", "access code"]
            .iter().map(|s| s.to_string()).collect()
    }

    /// 抽不到碼但命中關鍵字要算數 —— Netflix 的暫時存取碼就是這種，
    /// 而那正是這個專案存在的理由那封信。
    #[test]
    fn temporary_access_code_counts_even_without_a_number() {
        assert!(is_actionable(
            Some("您的 Netflix 暫時存取碼"),
            Some("我們收到下列裝置的暫時存取碼申請。"),
            false,
            &kw(),
            &[],
        ));
    }

    /// 新裝置通知與行銷信不該進驗證碼分頁 —— 對照真實收到的主旨。
    #[test]
    fn notifications_and_marketing_are_excluded() {
        for subject in [
            "有新裝置正在使用您的帳戶",
            "《大蟒蛇 4：血路斑駁》即將在9月1日星期二上線",
            "Grand Theft Auto VI：加長版預覽",
            "Hyena，這些 Netflix 影片即將上線",
        ] {
            assert!(
                !is_actionable(Some(subject), Some(""), false, &kw(), &[]),
                "{subject} 不該出現在驗證碼分頁"
            );
        }
    }

    /// 有碼一律算數，不必命中關鍵字 —— 主旨的寫法各平台不一樣。
    #[test]
    fn an_extracted_code_is_enough_on_its_own() {
        assert!(is_actionable(Some("Netflix：您的登入碼"), None, true, &[], &[]));
    }

    /// 排除字優先於一切，命中主旨就是不要。
    #[test]
    fn excludes_beat_everything() {
        assert!(!is_actionable(
            Some("您的驗證碼與本月電子報"),
            None,
            true,
            &kw(),
            &["電子報".into()],
        ));
    }

    /// 迴歸：排除字比對內文會把最該顯示的那封信擋掉。
    ///
    /// 真實案例 —— 排除字設「同戶」，而暫時存取碼信的內文正好在解釋規則時
    /// 寫著「在 Netflix 同戶裝置以外的裝置暫時使用」。比對內文的話，
    /// 這整套系統存在的理由那封信會被自己的說明文字擋掉。
    #[test]
    fn excludes_do_not_match_the_body() {
        let body = "此代碼僅限旅行用或在 Netflix 同戶裝置以外的裝置暫時使用，請勿傳給其他人。";
        assert!(
            is_actionable(Some("您的 Netflix 暫時存取碼"), Some(body), false, &kw(), &["同戶".into()]),
            "內文順帶提到排除字，不該讓整封信消失"
        );
        // 主旨真的在宣告這是那類信時才排除
        assert!(!is_actionable(Some("關於同戶裝置的說明"), Some(body), false, &kw(), &["同戶".into()]));
    }

    /// 關鍵字仍然比對內文 —— 那是包含條件，多命中只是多顯示一封。
    #[test]
    fn keywords_still_match_the_body() {
        assert!(is_actionable(Some("Netflix"), Some("您的驗證碼是 1234"), false, &kw(), &[]));
    }

    /// 沒設關鍵字時退回舊行為：只有抽得到碼的才算。
    #[test]
    fn empty_keywords_fall_back_to_code_only() {
        assert!(!is_actionable(Some("您的 Netflix 暫時存取碼"), None, false, &[], &[]));
        assert!(is_actionable(Some("任何主旨"), None, true, &[], &[]));
    }

    /// 主要連結要跳過頁尾樣板 —— 實測一封信 10 個連結，8 個是頁尾。
    #[test]
    fn primary_link_skips_the_footer() {
        let links: Vec<String> = [
            "https://www.netflix.com/account/travel/verify?nftoken=abc",
            "https://www.netflix.com/ManageAccountAccess?g=1",
            "https://help.netflix.com/help?g=1",
            "https://www.netflix.com/TermsOfUse?g=1",
            "https://www.netflix.com/PrivacyPolicy?g=1",
            "https://www.netflix.com/browse?g=1",
        ].iter().map(|s| s.to_string()).collect();

        assert_eq!(
            primary_link(&links).as_deref(),
            Some("https://www.netflix.com/account/travel/verify?nftoken=abc")
        );
    }

    /// 全部都是樣板時回 None，而不是硬挑一個頁尾連結給人按。
    #[test]
    fn primary_link_is_none_when_only_boilerplate() {
        let links: Vec<String> =
            ["https://help.netflix.com/help", "https://www.netflix.com/PrivacyPolicy"]
                .iter().map(|s| s.to_string()).collect();
        assert_eq!(primary_link(&links), None);
        assert_eq!(primary_link(&[]), None);
    }

    #[test]
    fn strips_html_and_script() {
        let t = html_to_text("<style>.a{color:red}</style><p>您的驗證碼</p><div>9182</div>");
        assert!(!t.contains("color"));
        assert!(t.contains("9182"));
        assert_eq!(extract_code(&t), Some("9182".into()));
    }

    /// 純文字段是空殼、驗證碼只在 HTML 裡。
    #[test]
    fn finds_code_when_only_html_has_it() {
        let raw = b"From: Netflix <info@account.netflix.com>\r\n\
Subject: Netflix\r\n\
Content-Type: multipart/alternative; boundary=\"b1\"\r\n\
\r\n\
--b1\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Please use the HTML version\r\n\
--b1\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body><p>Your verification code is</p><div>4821</div></body></html>\r\n\
--b1--\r\n";
        let p = parse(raw, "mx.cloudflare.net");
        assert_eq!(p.code, Some("4821".into()), "純文字段是空殼時要能從 HTML 抽到碼");
    }

    // ── 以下三個取自真實信件 ──

    /// 中文整句沒有空格時仍要抽得到碼。
    #[test]
    fn handles_cjk_without_spaces() {
        assert_eq!(extract_code("您的驗證碼：123456，15 分鐘內有效"), Some("123456".into()));
        assert_eq!(extract_code("驗證碼(482155)"), Some("482155".into()));
    }

    #[test]
    fn rejects_uuid_fragment() {
        // Netflix 通知信尾的追蹤字串，該信無驗證碼，正確答案是 None
        let t = "請驗證這是您本人。\nSRC: 5F639529_1be6e299-04c0-44eb-8300-9276379289a5_zh-TW_TW";
        assert_eq!(extract_code(t), None, "UUID 片段不該被當成驗證碼");
    }

    #[test]
    fn rejects_year_in_date() {
        // Disney+ 登入活動通知，同樣沒有驗證碼
        let t = "為了驗證是否為您本人：\n時間： 2026 年 08 月 09 日 AM12:39\n© 2026 迪士尼";
        assert_eq!(extract_code(t), None, "日期裡的年份不該被當成驗證碼");
    }

    #[test]
    fn decodes_numeric_entities() {
        // 未解碼的 HTML 實體會產生假候選
        let t = html_to_text("<p>您的驗證碼</p>&#8199;&#847;&#8199;&#847;<div>4821</div>");
        assert!(!t.contains("8199"), "數字型實體必須被解碼，否則變成假候選");
        assert_eq!(extract_code(&t), Some("4821".into()));
    }

    #[test]
    fn real_disney_otp_still_works() {
        let t = "使用這組驗證碼來驗證您的 MyDisney 帳戶。這組驗證碼將在 15 分鐘後失效。\n878688\n";
        assert_eq!(extract_code(t), Some("878688".into()));
    }

    #[test]
    fn extracts_links() {
        let l = extract_links(r#"go <a href="https://netflix.com/verify?x=1">here</a>"#);
        assert_eq!(l, vec!["https://netflix.com/verify?x=1"]);
    }

    /// 一個 URL 沒有長度上限，一封信就能帶 8 MiB 進 links 欄位與清單回應。
    #[test]
    fn oversized_links_are_dropped() {
        let huge = format!("https://x.example/{}", "a".repeat(MAX_LINK_LEN));
        let text = format!("{huge} https://ok.example/path");
        assert_eq!(extract_links(&text), vec!["https://ok.example/path"]);
    }

    // ── 寄件者驗證（網域取自實際日誌）────────────────

    fn allow() -> Vec<String> {
        vec!["netflix.com".into(), "disneyplus.com".into()]
    }

    fn auth_of(raw: &str) -> SenderAuth {
        let (dkim_domains, dmarc_from) = parse_auth_results(raw);
        SenderAuth { dkim_domains, dmarc_from, envelope_domain: None }
    }

    #[test]
    fn trusts_brand_dkim_behind_ses() {
        // SES 代寄會有兩條簽章，品牌那條要取到
        let a = auth_of(
            "mx.cloudflare.net; dkim=pass header.d=amazonses.com header.s=abc; \
             dkim=pass header.d=netflix.com header.s=s1; \
             spf=pass smtp.mailfrom=us-west-2.amazonses.com; \
             dmarc=pass header.from=netflix.com",
        );
        assert_eq!(a.dkim_domains, vec!["amazonses.com", "netflix.com"]);
        assert!(a.is_trusted(&allow()));
    }

    /// 信封網域內嵌 AWS 區域會隨區域切換而變，不得作為信任依據。
    #[test]
    fn survives_ses_region_change() {
        let a = auth_of(
            "mx.cloudflare.net; dkim=pass header.d=netflix.com; \
             spf=pass smtp.mailfrom=eu-west-1.amazonses.com; \
             dmarc=pass header.from=netflix.com",
        );
        assert!(a.is_trusted(&allow()), "換 AWS 區域不該影響判斷");
    }

    #[test]
    fn trusts_brand_subdomain() {
        let a = auth_of("mx.cloudflare.net; dkim=pass header.d=mail2.disneyplus.com; dmarc=pass header.from=disneyplus.com");
        assert!(a.is_trusted(&allow()));
    }

    /// 他人的 SES 帳號能通過 SPF/DKIM，但簽不出 d=netflix.com。
    #[test]
    fn rejects_attacker_own_ses_account() {
        let a = auth_of(
            "mx.cloudflare.net; dkim=pass header.d=amazonses.com; \
             spf=pass smtp.mailfrom=us-west-2.amazonses.com; dmarc=none",
        );
        assert!(!a.is_trusted(&allow()), "共用基礎設施網域不足以構成信任");
    }

    #[test]
    fn rejects_failed_dkim() {
        let a = auth_of("mx.cloudflare.net; dkim=fail header.d=netflix.com; spf=fail; dmarc=fail header.from=netflix.com");
        assert!(a.dkim_domains.is_empty());
        assert!(!a.is_trusted(&allow()));
    }

    /// 不同段的 header.d 與 pass 不得互相湊成通過。
    #[test]
    fn does_not_match_across_segments() {
        let a = auth_of("mx.cloudflare.net; dkim=fail header.d=attacker.com; spf=pass header.d=netflix.com");
        assert!(a.dkim_domains.is_empty(), "dkim=pass 與 header.d 必須在同一段");
        assert!(!a.is_trusted(&allow()));
    }

    #[test]
    fn rejects_missing_auth_results() {
        assert!(!auth_of("").is_trusted(&allow()));
    }

    /// 後綴比對須帶點，netflix.com.evil.com 不得通過。
    #[test]
    fn rejects_lookalike_domain() {
        let a = auth_of("mx.cloudflare.net; dkim=pass header.d=netflix.com.evil.com; dmarc=pass header.from=netflix.com.evil.com");
        assert!(!a.is_trusted(&allow()));
    }

    /// MTA 會把寄件者可控的字串原樣抄進自己的 AR 表頭，最常見的是
    /// `smtp.mailfrom=` 的信封位址。`dkim=pass.header.d=netflix.com@evil.com`
    /// 是一個合法的信封位址 —— 判決只能是每段的**第一個 token**，否則寄件者
    /// 光靠挑一個信箱名字就能替自己蓋章。
    #[test]
    fn sender_controlled_properties_cannot_smuggle_a_verdict() {
        let a = auth_of(
            "mx.cloudflare.net; dkim=none; \
             spf=fail smtp.mailfrom=dkim=pass.header.d=netflix.com@evil.com",
        );
        assert!(a.dkim_domains.is_empty(), "smtp.mailfrom 的內容不是判決");
        assert!(!a.is_trusted(&allow()));

        let a = auth_of(
            "mx.cloudflare.net; dmarc=none; \
             spf=fail smtp.mailfrom=dmarc=pass.header.from=netflix.com@evil.com",
        );
        assert_eq!(a.dmarc_from, None, "smtp.mailfrom 的內容不是判決");
        assert!(!a.is_trusted(&allow()));
    }

    /// 括號內是 CFWS 註解（常見於 `dkim=pass (1024-bit key)`），裡面可以是
    /// 任何字元，所以它也是一條夾帶路徑。
    #[test]
    fn a_verdict_inside_a_comment_is_not_a_verdict() {
        let a = auth_of("mx.cloudflare.net; dkim=none (dkim=pass header.d=netflix.com)");
        assert!(a.dkim_domains.is_empty());
        assert!(!a.is_trusted(&allow()));
    }

    /// 收緊之後，真實表頭（含 `header.i=` 與 `smtp.mailfrom=` 等額外欄位）
    /// 仍要判成通過 —— 這條是誤殺的守門員。
    #[test]
    fn a_real_header_still_passes_after_token_anchoring() {
        let a = auth_of(
            "mx.cloudflare.net; dkim=pass header.d=netflix.com header.i=@netflix.com; \
             spf=pass smtp.mailfrom=bounce@netflix.com",
        );
        assert_eq!(a.dkim_domains, vec!["netflix.com"]);
        assert!(a.is_trusted(&allow()));
    }

    /// `;`、空白、`=` 都是 RFC 5321 引號本地部的合法字元，所以一個合法的
    /// 信封位址可以在表頭裡「長出」一個新段落。引號內的東西不管長什麼樣，
    /// 都不能開出新的段落或新的 token。
    #[test]
    fn a_quoted_property_value_cannot_open_a_segment() {
        let a = auth_of(
            r#"mx.cloudflare.net; spf=pass smtp.mailfrom="; dkim=pass header.d=netflix.com"@evil.com"#,
        );
        assert!(a.dkim_domains.is_empty(), "引號內的內容不能自成一段");
        assert!(!a.is_trusted(&allow()));

        // `\"` 是跳脫過的引號，不結束字串 —— 否則從這裡就能溜出去
        let a = auth_of(
            r#"mx.cloudflare.net; spf=pass smtp.mailfrom="\"; dkim=pass header.d=netflix.com"@evil.com"#,
        );
        assert!(a.dkim_domains.is_empty(), "跳脫的引號不結束字串");
        assert!(!a.is_trusted(&allow()));

        // token 的分隔不只有空格
        let a = auth_of(
            "mx.cloudflare.net; spf=pass smtp.mailfrom=\";\tdkim=pass\theader.d=netflix.com\"@evil.com",
        );
        assert!(a.dkim_domains.is_empty(), "tab 也是分隔字元");
        assert!(!a.is_trusted(&allow()));
    }

    /// 反過來也要成立：RFC 8601 的 `reason=` 是引號字串，裡面本來就可能有
    /// `;`。把引號內的分號當成段落分隔，真信會被判成沒通過。
    #[test]
    fn a_quoted_reason_with_a_semicolon_is_still_a_pass() {
        let a = auth_of(
            r#"mx.cloudflare.net; dkim=pass reason="key retrieved; ok" header.d=netflix.com"#,
        );
        assert_eq!(a.dkim_domains, vec!["netflix.com"]);
        assert!(a.is_trusted(&allow()));
    }

    /// RFC 8601 的方法可以帶版本號：`method [ "/" version ] "=" result`。
    #[test]
    fn a_method_with_a_version_still_passes() {
        let a = auth_of("mx.cloudflare.net; dkim/1=pass header.d=netflix.com");
        assert!(a.is_trusted(&allow()));
    }

    #[test]
    fn parses_auth_from_real_message() {
        let raw = b"From: Netflix <info@account.netflix.com>\r\n\
Authentication-Results: mx.cloudflare.net;\r\n\
\tdkim=pass header.d=netflix.com header.s=s1;\r\n\
\tspf=pass smtp.mailfrom=us-west-2.amazonses.com;\r\n\
\tdmarc=pass header.from=netflix.com\r\n\
Subject: \xe9\xa9\x97\xe8\xad\x89\xe7\xa2\xbc\r\n\
\r\n\
\xe6\x82\xa8\xe7\x9a\x84\xe9\xa9\x97\xe8\xad\x89\xe7\xa2\xbc\xef\xbc\x9a123456\r\n";
        let p = parse(raw, "mx.cloudflare.net");
        assert!(p.auth.is_trusted(&allow()), "折行的表頭要能正確解析");
        assert_eq!(p.auth.envelope_domain.as_deref(), Some("account.netflix.com"));
        assert_eq!(p.code, Some("123456".into()));
    }

    fn raw_with(headers: &str) -> Vec<u8> {
        format!("{headers}\r\nFrom: a@netflix.com\r\nSubject: x\r\n\r\nbody\r\n").into_bytes()
    }

    /// 寄件者可以自己塞任意同名表頭，但收信端（Cloudflare）的表頭永遠加在
    /// 最頂端。只看第一個，而且它的 authserv-id 要是我們自己的。
    #[test]
    fn forged_auth_results_below_the_real_one_are_ignored() {
        let m = parse(
            &raw_with(
                "Authentication-Results: mx.cloudflare.net; dkim=fail header.d=netflix.com\r\n\
                 Authentication-Results: mx.cloudflare.net; dkim=pass header.d=netflix.com",
            ),
            "mx.cloudflare.net",
        );
        assert!(!m.auth.is_trusted(&["netflix.com".into()]));
    }

    #[test]
    fn auth_results_from_an_unknown_authserv_are_ignored() {
        let m = parse(
            &raw_with("Authentication-Results: evil.example; dkim=pass header.d=netflix.com"),
            "mx.cloudflare.net",
        );
        assert!(m.auth.dkim_domains.is_empty());
    }

    /// 第一個表頭不是我們的收信端時就到此為止，不會往下找到一個「合格」的。
    #[test]
    fn a_forged_header_below_a_non_matching_first_one_is_ignored() {
        let m = parse(
            &raw_with(
                "Authentication-Results: other.example; dkim=none\r\n\
                 Authentication-Results: mx.cloudflare.net; dkim=pass header.d=netflix.com",
            ),
            "mx.cloudflare.net",
        );
        assert!(!m.auth.is_trusted(&["netflix.com".into()]));
    }

    /// 第一個表頭的值是空的，也仍然是「第一個」—— 不得跳過它去讀第二個，
    /// 否則寄件者只要在信頂塞一行空的同名表頭就能讓自己那行被採信。
    #[test]
    fn an_empty_first_header_does_not_fall_through_to_the_second() {
        let m = parse(
            &raw_with(
                "Authentication-Results:\r\n\
                 Authentication-Results: mx.cloudflare.net; dkim=pass header.d=netflix.com",
            ),
            "mx.cloudflare.net",
        );
        assert!(!m.auth.is_trusted(&["netflix.com".into()]));
    }

    /// 完全沒有 Authentication-Results 的信落在「未通過」。
    #[test]
    fn a_message_with_no_authentication_results_is_not_trusted() {
        let m = parse(&raw_with(""), "mx.cloudflare.net");
        assert!(!m.auth.is_trusted(&["netflix.com".into()]));
    }

    /// authserv-id 前面可以合法地帶一段 CFWS 註解。那是註解，不是別人的
    /// 收信端 —— 不能因此把整封信扣住。
    #[test]
    fn a_leading_comment_before_the_authserv_id_is_fine() {
        let m = parse(
            &raw_with(
                "Authentication-Results: (added by cf) mx.cloudflare.net; \
                 dkim=pass header.d=netflix.com",
            ),
            "mx.cloudflare.net",
        );
        assert!(m.auth.is_trusted(&["netflix.com".into()]));
    }

    /// 表頭名稱大小寫不敏感（RFC 5322），挑表頭時不能因此漏掉。
    #[test]
    fn the_header_name_is_matched_case_insensitively() {
        let m = parse(
            &raw_with("AUTHENTICATION-RESULTS: mx.cloudflare.net; dkim=pass header.d=netflix.com"),
            "mx.cloudflare.net",
        );
        assert!(m.auth.is_trusted(&["netflix.com".into()]));
    }

    /// RFC 8601 允許 authserv-id 後面帶版本號。
    #[test]
    fn the_real_authserv_still_passes() {
        let m = parse(
            &raw_with("Authentication-Results: MX.Cloudflare.NET 1; dkim=pass header.d=netflix.com"),
            "mx.cloudflare.net",
        );
        assert!(m.auth.is_trusted(&["netflix.com".into()]));
    }

    /// 解析失敗時沒有表頭可讀，必須落在「未通過」。
    #[test]
    fn unparseable_mail_is_not_trusted() {
        let p = parse(b"\xff\xfe not a real message at all", "mx.cloudflare.net");
        assert!(!p.auth.is_trusted(&allow()));
    }

    /// `İ`（U+0130）小寫化後變成兩個 code point、byte 數改變 —— 以前拿原字串
    /// 的 byte 位移去切小寫字串，會落在字元中間而 panic。
    #[test]
    fn html_with_length_changing_lowercase_does_not_panic() {
        let t = html_to_text("İ<script>x</script><p>ok</p>");
        assert!(t.contains("ok"));
        assert!(!t.contains("x"), "script 內容要整段丟掉");
    }

    /// 實體視窗以前固定切 12 bytes，第 12 個 byte 落在多位元字元中間就 panic。
    #[test]
    fn entity_window_never_splits_a_multibyte_char() {
        assert_eq!(decode_entities("&a日日日日;"), "&a日日日日;");
        assert_eq!(decode_entities("&amp;日"), "&日");
    }

    /// 整個解析器對任意 Unicode 都不得 unwind。
    #[test]
    fn parser_survives_hostile_unicode_html() {
        for s in ["İ<", "<İ>", "&İİİİ;", "<p>İ</p>", "&#x1F600;İ<br", "ﬀ<style>İ</style>ﬀ", "<İ"] {
            let _ = html_to_text(s);
            let _ = decode_entities(s);
        }
    }

    /// 迴歸：跳過 script/style 區塊後，原本會誤吞到「下一個」`>` 為止，
    /// 把區塊之後的內容一起吃掉。
    #[test]
    fn text_after_a_script_or_style_block_is_kept() {
        let t = html_to_text("<style>x{}</style>487261 is your code");
        assert!(t.contains("487261"), "style 區塊之後的內容不該被吞掉");

        let t = html_to_text("<script>a</script>KEEP<p>y</p>");
        assert!(t.contains("KEEP"), "script 區塊之後的內容不該被吞掉");
        assert!(t.contains('y'));
        assert!(!t.contains('a'), "script 內容本身仍要整段丟掉");
    }

    /// 主旨、寄件者、Message-ID 沒有上限的話，每封信都能帶幾 MB 的表頭進資料庫。
    ///
    /// 主旨截短就好；位址與 Message-ID 只能整個拒收 —— 截短出來的位址會多出
    /// 一個它本來沒有的網域，截短出來的 Message-ID 會撞掉別封信。
    #[test]
    fn header_metadata_is_capped() {
        let long = "x".repeat(5000);
        let raw = format!(
            "From: {long}@example.com\r\nTo: {long}@share.example.com\r\nSubject: {long}\r\nMessage-ID: <{long}@x>\r\n\r\nhi\r\n"
        );
        let m = parse(raw.as_bytes(), "mx.cloudflare.net");
        assert_eq!(m.subject.unwrap().chars().count(), MAX_SUBJECT_CHARS, "主旨截到上限");
        assert_eq!(m.sender, None, "超長寄件者整個不採信，不是切一半");
        assert_eq!(m.recipient, None, "收件者同理");
        assert_eq!(m.message_id, None, "去重鍵寧可沒有，也不要一個截短的");
        assert_eq!(m.auth.envelope_domain, None, "位址都拒收了，網域不該還留著");
    }

    /// 正常長度的表頭一個都不能被上限誤傷。
    #[test]
    fn normal_headers_survive_the_caps() {
        let raw = "From: info@account.netflix.com\r\nTo: netflix@share.example.com\r\n\
                   Subject: 您的登入驗證碼\r\nMessage-ID: <abc123@netflix.com>\r\n\r\nhi\r\n";
        let m = parse(raw.as_bytes(), "mx.cloudflare.net");
        assert_eq!(m.sender.as_deref(), Some("info@account.netflix.com"));
        assert_eq!(m.recipient.as_deref(), Some("netflix@share.example.com"));
        assert_eq!(m.subject.as_deref(), Some("您的登入驗證碼"));
        assert_eq!(m.message_id.as_deref(), Some("abc123@netflix.com"));
        assert_eq!(m.auth.envelope_domain.as_deref(), Some("account.netflix.com"));
    }
}
