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
});

test("4xx 與壞 JSON 是拒收，5xx 是不可用", async () => {
  expect((await classifyResponse(new Response("", { status: 401 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("", { status: 422 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("not json", { status: 200 }))).kind).toBe("rejected");
  expect((await classifyResponse(new Response("", { status: 503 }))).kind).toBe("unavailable");
  expect((await classifyResponse(new Response('{"forward_to":[]}', { status: 200 }))).kind).toBe("ok");
});
