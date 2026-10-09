import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import vm from "node:vm"

const source = (await readFile(new URL("../overrides/controllers/notifications_controller.js", import.meta.url), "utf8"))
  .replace(/^import .*\n/gm, "")
  .replace("export default class", "globalThis.Notifications = class")

function deferred() {
  let resolve
  const promise = new Promise(done => { resolve = done })
  return { promise, resolve }
}

function fixture({ permission = "default", key = "AQID", available = true, existing = false, postResult = { ok: true } } = {}) {
  const calls = []
  const subscription = {
    toJSON: () => ({ endpoint: "https://push.example/sub", keys: { p256dh: "public", auth: "auth" } }),
    unsubscribe: async () => { calls.push("unsubscribe") },
  }
  const registration = {
    active: true,
    pushManager: {
      getSubscription: async () => { calls.push("getSubscription"); return existing ? subscription : null },
      subscribe: async () => { calls.push("subscribe"); return subscription },
    },
  }
  const Notification = {
    permission,
    requestPermission: async () => { calls.push("requestPermission"); Notification.permission = "granted"; return "granted" },
  }
  const navigator = { serviceWorker: {
    getRegistration: async () => { calls.push("getRegistration"); return registration },
    register: async () => { calls.push("register"); return registration },
    ready: Promise.resolve(registration),
  } }
  const window = {
    Notification: available ? Notification : null,
    PushManager: available ? class {} : null,
    isSecureContext: true,
    location: { origin: "https://ember.example" },
    matchMedia: () => ({ matches: false }),
    atob,
  }
  const context = vm.createContext({
    Controller: class { dispatch(name) { calls.push(name) } },
    navigator, window, Notification,
    document: { querySelector: () => ({ content: key }) },
    pageIsTurboPreview: () => false,
    onNextEventLoopTick: callback => callback(),
    getCookie: () => true,
    setCookie: () => {},
    post: async (_, options) => { calls.push("post"); assert.equal(JSON.parse(options.body).push_subscription.p256dh_key, "public"); return postResult },
  })
  vm.runInContext(source, context)
  const controller = new context.Notifications()
  Object.assign(controller, {
    subscriptionsUrlValue: "/users/me/push_subscriptions",
    hasBellTarget: true,
    bellTarget: { disabled: false, classList: { add() {}, remove() {} }, setAttribute() {}, removeAttribute() {} },
    detailsTargets: [],
    noticeTitleTarget: { textContent: "" },
    noticeMessageTarget: { textContent: "" },
    helpTarget: { hidden: true },
    notAllowedNoticeTarget: { open: false, showModal() { this.open = true; calls.push("notice") } },
  })
  return { controller, calls, Notification, navigator, registration, window }
}

test("permission is requested in the click before asynchronous worker work, and ready waits for saved subscription", async () => {
  const saving = deferred()
  const f = fixture({ postResult: saving.promise })
  await f.controller.connect()
  assert.deepEqual(f.calls, [])
  const clicked = f.controller.attemptToSubscribe()
  assert.deepEqual(f.calls, ["requestPermission"])
  assert.equal(f.controller.bellTarget.disabled, true)
  await new Promise(setImmediate)
  assert.deepEqual(f.calls, ["requestPermission", "getRegistration", "getSubscription", "subscribe", "post"])
  saving.resolve({ ok: true })
  await clicked
  assert.equal(f.calls.at(-1), "ready")
  assert.equal(f.controller.bellTarget.disabled, false)
})

test("denied, unsupported and unconfigured browsers give actionable feedback without requesting permission", async () => {
  for (const [options, title] of [
    [{ permission: "denied" }, "Notifications are blocked"],
    [{ available: false }, "Notifications aren’t available here"],
    [{ key: "" }, "Notifications aren’t configured"],
  ]) {
    const f = fixture(options)
    await f.controller.connect()
    await f.controller.attemptToSubscribe()
    assert.equal(f.controller.noticeTitleTarget.textContent, title)
    assert.deepEqual(f.calls, ["notice"])
    assert.ok(f.controller.noticeMessageTarget.textContent.length > 20)
  }
})

test("dismissed permission never registers a worker or reports success", async () => {
  const f = fixture()
  f.Notification.requestPermission = async () => "default"
  await f.controller.connect()
  await f.controller.attemptToSubscribe()
  assert.deepEqual(f.calls, ["notice"])
  assert.equal(f.controller.bellTarget.disabled, false)
})

test("a failed server save rolls back a new browser subscription and remains retryable", async () => {
  const f = fixture({ postResult: { ok: false } })
  await f.controller.connect()
  await f.controller.attemptToSubscribe()
  assert.ok(f.calls.includes("unsubscribe"))
  assert.ok(!f.calls.includes("ready"))
  assert.equal(f.controller.noticeTitleTarget.textContent, "Couldn’t enable notifications")
  assert.equal(f.controller.bellTarget.disabled, false)
})

test("duplicate clicks do not duplicate subscriptions, and existing subscriptions are reused", async () => {
  const f = fixture({ existing: true })
  await f.controller.connect()
  await Promise.all([f.controller.attemptToSubscribe(), f.controller.attemptToSubscribe()])
  assert.equal(f.calls.filter(call => call === "requestPermission").length, 1)
  assert.equal(f.calls.filter(call => call === "post").length, 1)
  assert.ok(!f.calls.includes("subscribe"))
})

test("connection failure is handled and permission is rechecked after navigation", async () => {
  const f = fixture({ permission: "granted", existing: true })
  await f.controller.connect()
  assert.equal(f.calls.at(-1), "ready")
  f.controller.disconnect()
  f.calls.length = 0
  f.Notification.permission = "denied"
  await f.controller.connect()
  assert.deepEqual(f.calls, [])
  f.Notification.permission = "granted"
  f.navigator.serviceWorker.getRegistration = async () => { throw new Error("worker unavailable") }
  await f.controller.attemptToSubscribe()
  assert.equal(f.controller.noticeTitleTarget.textContent, "Couldn’t enable notifications")
})

test("new worker activation is awaited before subscribing", async () => {
  const f = fixture()
  f.navigator.serviceWorker.getRegistration = async () => undefined
  f.navigator.serviceWorker.register = async () => ({ active: null })
  const ready = deferred()
  f.navigator.serviceWorker.ready = ready.promise
  await f.controller.connect()
  const clicked = f.controller.attemptToSubscribe()
  await new Promise(setImmediate)
  assert.ok(!f.calls.includes("subscribe"))
  ready.resolve(f.registration)
  await clicked
  assert.equal(f.calls.at(-1), "ready")
})
