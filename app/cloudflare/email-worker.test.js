import { test, expect } from "bun:test";
import { classifyResponse, decideTargets } from "./email-worker.js";

const env = {
  FALLBACK_TO: "me@x",
  FORWARD_MAP: JSON.stringify({ "netflix@share.x": ["fam@x"] }),
};

test("面板拒收：只轉給 FALLBACK_TO，不走 FORWARD_MAP", () => {
  expect(decideTargets({ kind: "rejected", status: 422 }, "netflix@share.x", env)).toEqual(["me@x"]);
});

test("面板不可用或未設定：才走 FORWARD_MAP", () => {
  expect(decideTargets({ kind: "unavailable" }, "netflix@share.x", env)).toEqual(["me@x", "fam@x"]);
  expect(decideTargets({ kind: "unconfigured" }, "netflix@share.x", env)).toEqual(["me@x", "fam@x"]);
});

test("面板回覆：照單轉", () => {
  expect(decideTargets({ kind: "ok", panel: { forward_to: ["a@x"] } }, "netflix@share.x", env))
    .toEqual(["me@x", "a@x"]);

  // forward_to 不是陣列時只轉 FALLBACK_TO —— 展開非陣列會拋，而這裡在
  // message.forward() 之前、任何 try 之外，一拋整封信就不見了
  for (const junk of [5, {}, true]) {
    expect(decideTargets({ kind: "ok", panel: { forward_to: junk } }, "netflix@share.x", env))
      .toEqual(["me@x"]);
  }
  // 字串可迭代，展開會被拆成一個個字元
  expect(decideTargets({ kind: "ok", panel: { forward_to: "a@x" } }, "netflix@share.x", env))
    .toEqual(["me@x"]);
});

test("4xx 與壞 JSON 是拒收，5xx 是不可用", async () => {
  expect((await classifyResponse(new Response("", { status: 401 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("", { status: 422 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("not json", { status: 200 }))).kind).toBe("rejected");
  // 合法 JSON 但不是面板會回的物件：照 ok 走會在轉發前就拋例外，整封信不見
  expect((await classifyResponse(new Response("null", { status: 200 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("[]", { status: 200 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("", { status: 404 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("", { status: 503 }))).kind).toBe("unavailable");
  // 408／429 是「等一下再試」，不是拒收 —— 面板前面的 Tunnel 也可能自己回 429
  expect((await classifyResponse(new Response("", { status: 408 }))).kind).toBe("unavailable");
  expect((await classifyResponse(new Response("", { status: 429 }))).kind).toBe("unavailable");
  expect((await classifyResponse(new Response('{"forward_to":[]}', { status: 200 }))).kind).toBe("ok");
});
