/**
 * Service worker —— 只做推送通知，刻意不做離線快取。
 *
 * ⚠️ 不要加 fetch 快取：面板的內容是驗證碼，快取唯一的效果是讓人
 *    看到過期的碼還以為是新的。
 * ⚠️ 檔名固定（放 public/ 讓 Vite 原樣複製，不套 hash）—— 網址一變
 *    瀏覽器就當成另一支，舊的會繼續活著。必須從根目錄 /sw.js 送出。
 */

// 面板推來的酬載（見 Rust 的 push::Notification）
self.addEventListener("push", (event) => {
  if (!event.data) return;

  let n;
  try {
    n = event.data.json();
  } catch {
    return; // 解不開就不顯示，總比彈一則亂碼好
  }

  // iOS 忽略 icon 與 actions。照樣帶著 —— 不必為兩個平台分兩套酬載。
  const options = {
    body: n.body,
    icon: "/icon-192.png",
    badge: "/icon-192.png",
    tag: n.tag,
    // 少了它，新的碼會靜靜地換掉舊的那則
    renotify: true,
    data: { url: n.url || "/", code: n.code || null },
  };
  if (n.code) {
    options.actions = [{ action: "copy", title: "複製" }];
  }

  event.waitUntil(self.registration.showNotification(n.title, options));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const { url = "/", code = null } = event.notification.data || {};
  const wantsCopy = event.action === "copy" && code;

  event.waitUntil(
    (async () => {
      const clientList = await self.clients.matchAll({
        type: "window",
        includeUncontrolled: true,
      });

      // 已經開著就聚焦那一個，不要每次都開新視窗
      let client = clientList.find((c) => new URL(c.url).origin === self.location.origin);
      if (client) {
        await client.focus();
        if (client.url !== url && "navigate" in client) {
          client = (await client.navigate(url)) || client;
        }
      } else {
        client = await self.clients.openWindow(url);
      }

      // 剪貼簿在 worker 裡碰不到，請頁面代勞（要等它聚焦才寫得進去）
      if (wantsCopy && client) {
        client.postMessage({ type: "copy-code", code });
      }
    })()
  );
});
