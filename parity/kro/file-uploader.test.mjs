import assert from "node:assert/strict"
import { test } from "node:test"
import FileUploader, { MAX_FILE_SIZE } from "../../crates/assets/overrides/models/file_uploader.js"

class Request extends EventTarget {
  static latest
  upload = new EventTarget()
  headers = {}
  constructor() { super(); Request.latest = this }
  open(method, url) { this.method = method; this.url = url }
  setRequestHeader(name, value) { this.headers[name] = value }
  getResponseHeader() { return this.contentType }
  send(body) { this.body = body }
  finish(status, contentType = "text/vnd.turbo-stream.html") {
    Object.assign(this, { status, contentType, response: "<turbo-stream></turbo-stream>" })
    this.dispatchEvent(new Event("load"))
  }
}

function mockRequests(t) {
  const previous = globalThis.XMLHttpRequest
  globalThis.XMLHttpRequest = Request
  t.after(() => { globalThis.XMLHttpRequest = previous })
}

test("uploader enforces 250 MB before starting a request and reports real progress", async t => {
  mockRequests(t)
  assert.equal(MAX_FILE_SIZE, 250_000_000)
  Request.latest = null
  await assert.rejects(new FileUploader({ size: MAX_FILE_SIZE + 1 }, "/upload", "one", () => {}).upload(), /250 MB/)
  assert.equal(Request.latest, null)

  const file = new File(["hello"], "clip.mp4", { type: "video/mp4" })
  Object.defineProperty(file, "size", { value: MAX_FILE_SIZE })
  const progress = []
  const done = new FileUploader(file, "/upload", "one", (...args) => progress.push(args)).upload()
  const request = Request.latest
  request.upload.dispatchEvent(Object.assign(new Event("progress"), { lengthComputable: true, loaded: 429, total: 1000 }))
  assert.deepEqual(progress[0], [42, "one", file])
  assert.equal(request.headers.Accept, "text/vnd.turbo-stream.html")
  assert.equal(request.body.get("message[client_message_id]"), "one")
  request.finish(200)
  assert.match(await done, /turbo-stream/)
})

test("HTTP, network, cancellation, timeout, and sign-in redirects fail instead of looking complete", async t => {
  mockRequests(t)
  for (const [status, type, message] of [[413, "text/html", /250 MB/], [500, "text/html", /Upload failed/], [200, "text/html", /Upload failed/]]) {
    const done = new FileUploader(new File(["x"], "x.txt"), "/upload", "two", () => {}).upload()
    Request.latest.finish(status, type)
    await assert.rejects(done, message)
  }
  for (const [event, message] of [["error", /Connection lost/], ["abort", /canceled/], ["timeout", /timed out/]]) {
    const done = new FileUploader(new File(["x"], "x.txt"), "/upload", "two", () => {}).upload()
    Request.latest.dispatchEvent(new Event(event))
    await assert.rejects(done, message)
  }
})
