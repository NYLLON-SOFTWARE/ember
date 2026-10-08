import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import vm from "node:vm"

const maximumBytes = 5 * 1024 * 1024
const source = (await readFile(new URL("../overrides/controllers/svg_preview_controller.js", import.meta.url), "utf8"))
  .replace(/^import .*\n/gm, "")
  .replace("export default class", "globalThis.SvgPreview = class")

function deferred() {
  let resolve
  const promise = new Promise(done => { resolve = done })
  return { promise, resolve }
}

function fixture({ chunks = [new TextEncoder().encode('<svg xmlns="http://www.w3.org/2000/svg"/>')], header = null, ok = true } = {}) {
  const calls = []
  const observers = []
  let index = 0
  const reader = {
    read: async () => index < chunks.length ? { value: chunks[index++], done: false } : { done: true },
    cancel: async () => { calls.push("cancel") },
  }
  const response = {
    ok,
    headers: { get: () => header },
    body: { getReader: () => reader, cancel: reader.cancel },
  }
  const image = {
    src: "", hidden: true,
    getAttribute(name) { return this[name] || null },
    removeAttribute(name) { this[name] = "" },
    decode: async () => { calls.push("decode") },
  }
  const context = vm.createContext({
    Controller: class {},
    URL, Blob, AbortController,
    window: { location: { href: "https://matchbox.example/rooms/1", origin: "https://matchbox.example" } },
    fetch: async (url, options) => { calls.push(["fetch", url.href, options]); return response },
    IntersectionObserver: class {
      constructor(callback) { this.callback = callback; observers.push(this) }
      observe() { calls.push("observe") }
      disconnect() { this.disconnected = true }
    },
    FileReader: class {
      async readAsDataURL(blob) {
        this.result = `data:${blob.type};base64,${Buffer.from(await blob.arrayBuffer()).toString("base64")}`
        this.onload()
      }
    },
  })
  vm.runInContext(source, context)
  const controller = new context.SvgPreview()
  controller.imageTarget = image
  controller.urlValue = "/rails/active_storage/blobs/redirect/signed/drawing.svg?disposition=attachment"
  controller.element = {}
  return { controller, image, calls, observers, reader, response }
}

test("SVG previews wait for visibility and use only an image data URL", async () => {
  const f = fixture()
  f.controller.connect()
  assert.deepEqual(f.calls, ["observe"])
  f.observers[0].callback([{ isIntersecting: true }])
  await new Promise(setImmediate)
  assert.equal(f.image.hidden, false)
  assert.match(f.image.src, /^data:image\/svg\+xml;base64,/)
  assert.equal(f.observers[0].disconnected, true)
  assert.equal(f.calls.find(Array.isArray)[2].credentials, "same-origin")
})

test("exactly 5 MiB can decode, while oversized streams are cancelled without decoding", async () => {
  for (const size of [maximumBytes, maximumBytes + 1]) {
    const f = fixture({ chunks: [new Uint8Array(size)] })
    await f.controller.load(new AbortController().signal)
    assert.ok(f.calls.includes("cancel"))
    assert.equal(f.calls.includes("decode"), size === maximumBytes)
    assert.equal(f.image.hidden, size > maximumBytes)
  }
})

test("oversized headers cancel the response body before reading it", async () => {
  const f = fixture({ header: String(maximumBytes + 1) })
  await f.controller.load(new AbortController().signal)
  assert.ok(f.calls.includes("cancel"))
  assert.ok(!f.calls.includes("decode"))
  assert.equal(f.image.src, "")
})

test("cross-origin URLs never fetch and failed image decoding preserves the file fallback", async () => {
  const remote = fixture()
  remote.controller.urlValue = "https://untrusted.example/image.svg"
  await remote.controller.load(new AbortController().signal)
  assert.deepEqual(remote.calls, [])
  const invalid = fixture()
  invalid.image.decode = async () => { throw new Error("Invalid SVG") }
  await invalid.controller.load(new AbortController().signal)
  assert.equal(invalid.image.hidden, true)
  assert.equal(invalid.image.src, "")
})

test("disconnect aborts pending work and reconnect can load a fresh preview", async () => {
  const f = fixture()
  f.controller.connect()
  const oldAbort = f.controller.abort
  const decoding = deferred()
  f.image.decode = () => decoding.promise
  const loading = f.controller.load(oldAbort.signal)
  await new Promise(setImmediate)
  f.controller.disconnect()
  assert.equal(oldAbort.signal.aborted, true)
  assert.equal(f.observers[0].disconnected, true)
  f.controller.connect()
  assert.notEqual(f.controller.abort, oldAbort)
  assert.equal(f.image.src, "")
  decoding.resolve()
  await loading
  assert.equal(f.image.hidden, true, "old load cannot reveal an aborted image")
  f.image.decode = async () => {}
  await f.controller.load(f.controller.abort.signal)
  assert.equal(f.image.hidden, false)
})

test("Turbo-restored completed previews retain the image without another fetch", async () => {
  const f = fixture()
  f.controller.connect()
  await f.controller.load(f.controller.abort.signal)
  f.controller.disconnect()
  const src = f.image.src
  f.calls.length = 0
  f.controller.connect()
  assert.equal(f.image.src, src)
  assert.equal(f.image.hidden, false)
  assert.deepEqual(f.calls, [])
})

test("a queued observer callback from an old Turbo body cannot start work after reconnect", async () => {
  const f = fixture()
  f.controller.connect()
  const oldObserver = f.observers[0]
  f.controller.disconnect()
  f.controller.connect()
  f.calls.length = 0
  oldObserver.callback([{ isIntersecting: true }])
  await new Promise(setImmediate)
  assert.deepEqual(f.calls, [])
  assert.equal(f.observers[1].disconnected, undefined)
})
