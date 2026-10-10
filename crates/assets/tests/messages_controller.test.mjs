import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import vm from "node:vm"

async function source(path, name) {
  return (await readFile(new URL(path, import.meta.url), "utf8"))
    .replace(/^import .*\n/gm, "")
    .replace("export default class", `globalThis.${name} = class`)
}

const messagesSource = await source("../overrides/controllers/messages_controller.js", "Messages")
const clientSource = await source("../overrides/models/client_message.js", "ClientMessage")
const template = await readFile(new URL("../../views/templates/messages/_template.html", import.meta.url), "utf8")

function message({ id = "", clientId = "same", userId = "1", persisted = false, pending = false } = {}) {
  const classes = new Set(["message", "message--threaded"])
  const body = { innerHTML: "Original body" }
  return {
    id, body,
    dataset: {
      clientMessageId: clientId,
      userId,
      ...(persisted ? { messageId: id.replace("message_", "") } : {}),
      ...(pending ? { pendingMessage: "true" } : {}),
    },
    classList: {
      add: (...names) => names.forEach(name => classes.add(name)),
      remove: (...names) => names.forEach(name => classes.delete(name)),
      contains: name => classes.has(name),
    },
    querySelector: selector => selector === ".message__body-content" ? body : null,
    matches(selector) {
      if (selector === ".message") return true
      if (selector === "[data-pending-message]") return "pendingMessage" in this.dataset
      throw new Error(`Unexpected selector ${selector}`)
    },
    get nextElementSibling() {
      return this.parentElement?.children[this.parentElement.children.indexOf(this) + 1] || null
    },
    remove() {
      this.parentElement.children.splice(this.parentElement.children.indexOf(this), 1)
      this.parentElement = null
    },
  }
}

function fixture(nodes) {
  const formatted = []
  const container = {
    children: nodes,
    querySelectorAll(selector) {
      assert.equal(selector, "[data-pending-message]")
      return this.children.filter(node => node.matches(selector))
    },
  }
  for (const node of nodes) node.parentElement = container
  const document = { getElementById: id => container.children.find(node => node.id === id) || null }
  const context = vm.createContext({
    Controller: class {},
    Current: { user: { id: 1 } },
    MessageFormatter: class { format(node) { formatted.push(node) } },
    ThreadStyle: { thread: "thread" },
    document,
  })
  vm.runInContext(messagesSource, context)
  vm.runInContext(clientSource, context)
  const controller = new context.Messages()
  Object.assign(controller, {
    messagesTarget: container,
    firstOfDayClass: "message--first-of-day",
    formattedClass: "message--formatted",
    meClass: "message--me",
    mentionedClass: "message--mentioned",
    threadedClass: "message--threaded",
  })
  controller.initialize()
  const client = new context.ClientMessage({ innerHTML: template })
  return { controller, client, container, formatted }
}

test("persisted acknowledgments reconcile only the current user's matching pending rows", () => {
  const pending = message({ id: "pending_message_same", pending: true })
  const next = message({ id: "message_8", persisted: true, clientId: "other" })
  const otherPending = message({ id: "other-pending", pending: true, userId: "2" })
  const victim = message({ id: "message_9", persisted: true, userId: "2" })
  const received = message({ id: "message_10", persisted: true })
  const f = fixture([pending, next, otherPending, victim, received])
  f.controller.messageTargetConnected(received)
  assert.deepEqual(f.container.children, [next, otherPending, victim, received])
  assert.ok(f.formatted.includes(next), "the row after a removed placeholder is reformatted")
  assert.equal(next.classList.contains("message--threaded"), false)
  f.controller.messageTargetConnected(received)
  assert.deepEqual(f.container.children, [next, otherPending, victim, received], "duplicate HTTP/Cable acknowledgment is harmless")
})

test("another user's reused correlation cannot remove our optimistic message", () => {
  const pending = message({ id: "pending_message_same", pending: true })
  const foreign = message({ id: "message_10", persisted: true, userId: "2" })
  const f = fixture([pending, foreign])
  f.controller.messageTargetConnected(foreign)
  assert.deepEqual(f.container.children, [pending, foreign])
  f.controller.messageTargetConnected(pending)
  assert.deepEqual(f.container.children, [pending, foreign], "a pending row never reconciles other rows")
})

test("correlation matching treats selector syntax literally and stays within the room", () => {
  const clientId = 'a"] [data-pending-message], [data-user-id="2'
  const pending = message({ id: "pending-special", pending: true, clientId })
  const unrelated = message({ id: "pending-other", pending: true, clientId: "other" })
  const received = message({ id: "message_11", persisted: true, clientId })
  const f = fixture([pending, unrelated, received])
  f.controller.messageTargetConnected(received)
  assert.deepEqual(f.container.children, [unrelated, received])
})

test("upload progress and failures use only the pending namespace, even for numeric correlations", () => {
  const persisted = message({ id: "message_23", persisted: true, clientId: "23" })
  const pending = message({ id: "pending_message_23", pending: true, clientId: "23" })
  const f = fixture([persisted, pending])
  f.client.update("23", "Uploading")
  f.client.failed("23")
  assert.equal(pending.body.innerHTML, "Uploading")
  assert.equal(pending.classList.contains("message--failed"), true)
  assert.equal(persisted.body.innerHTML, "Original body")
  assert.equal(persisted.classList.contains("message--failed"), false)
  pending.remove()
  f.client.update("23", "Late upload failure")
  f.client.failed("23")
  assert.equal(persisted.body.innerHTML, "Original body", "an acknowledgment before an upload error preserves the real message")
})

test("optimistic text, emoji and sound messages retain content and correlation metadata", () => {
  const f = fixture([])
  const html = f.client.render("23", { value: "<p>Hello</p>", toString: () => "Hello" })
  assert.ok(html.includes('id="pending_message_23"'))
  assert.ok(html.includes('data-client-message-id="23"'))
  assert.ok(html.includes('data-pending-message="true"'))
  assert.ok(html.includes('<div class="lexxy-content"><p>Hello</p></div>'))
  assert.ok(f.client.render("emoji", "😀").includes("message--emoji"))
  assert.ok(f.client.render("sound", "/play rimshot").includes("Playing rimshot…"))
})
