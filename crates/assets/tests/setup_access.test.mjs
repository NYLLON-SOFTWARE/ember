import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import vm from "node:vm"

const source = await readFile(new URL("../overrides/setup_access.js", import.meta.url), "utf8")
const token = "a".repeat(64)

async function open({ protocol = "https:", fragment = `#token=${token}`, response = { status: 204 }, failure = false } = {}) {
  const events = []
  const status = { textContent: "Opening setup" }
  const context = vm.createContext({
    URLSearchParams,
    location: {
      protocol,
      pathname: "/first_run/access",
      hash: fragment,
      replace: path => events.push({ action: "navigate", path }),
    },
    history: { replaceState: (state, title, path) => events.push({ action: "clear", path }) },
    document: { getElementById: id => { assert.equal(id, "status"); return status } },
    fetch: async (path, options) => {
      events.push({ action: "exchange", path, options })
      if (failure) throw new Error("offline")
      return response
    },
  })
  await vm.runInContext(source, context)
  return { events, status }
}

test("private fragment is removed before exchanging it only in a same-origin POST body", async () => {
  const { events } = await open()
  assert.equal(events[0].action, "clear")
  assert.equal(events[0].path, "/first_run/access")
  const exchange = events[1]
  assert.equal(exchange.action, "exchange")
  assert.equal(exchange.path, "/first_run/access")
  assert.equal(exchange.options.method, "POST")
  assert.equal(exchange.options.body.get("token"), token)
  assert.equal(exchange.options.credentials, "same-origin")
  assert.equal(exchange.options.cache, "no-store")
  assert.equal(exchange.options.redirect, "manual")
  assert.deepEqual(events[2], { action: "navigate", path: "/first_run" })
  assert.ok(events.filter(event => event.path).every(event => !event.path.includes(token)))
})

test("insecure or incomplete links never send a token", async () => {
  for (const options of [{ protocol: "http:" }, { fragment: "" }, { fragment: "#token=bad" }]) {
    const { events, status } = await open(options)
    assert.deepEqual(events, [{ action: "clear", path: "/first_run/access" }])
    assert.ok(status.textContent.includes("HTTPS"))
    assert.ok(!status.textContent.includes(token))
  }
})

test("denied and failed exchanges remain on the token-free page without reflecting secrets", async () => {
  for (const options of [{ response: { status: 403 } }, { failure: true }]) {
    const { events, status } = await open(options)
    assert.ok(events.every(event => event.action !== "navigate"))
    assert.ok(!status.textContent.includes(token))
  }
})

test("already completed setup returns to the application", async () => {
  const { events } = await open({ response: { status: 0, type: "opaqueredirect" } })
  assert.deepEqual(events.at(-1), { action: "navigate", path: "/" })
})
