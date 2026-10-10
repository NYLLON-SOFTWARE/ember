import assert from "node:assert/strict"
import { after, before, test } from "node:test"
import { spawn } from "node:child_process"
import { once } from "node:events"
import fs from "node:fs/promises"
import net from "node:net"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { chromium } from "playwright"

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..")
const binary = path.resolve(process.env.EMBER_BIN || process.env.MATCHBOX_BIN || process.env.CAMPFIRE_BIN || path.join(repo, "target/debug/ember"))
const artifacts = path.join(repo, "parity/out/kro")
const password = "ember-workspace-test-password"
const sameOrigin = { "Sec-Fetch-Site": "same-origin" }
let browser
let pageNumber = 0

before(async () => {
  await fs.access(binary)
  await fs.mkdir(artifacts, { recursive: true })
  browser = await chromium.launch()
})
after(async () => { await browser?.close() })

async function capture(page, options) {
  // Failure captures must finish even if teardown interrupted an image or animation.
  await page.evaluate(() => Promise.race([(async () => {
    const animations = document.getAnimations().filter((animation) => Number.isFinite(animation.effect?.getComputedTiming().endTime))
    await Promise.all(animations.map((animation) => animation.finished.catch(() => {})))
    await Promise.all([...document.images].filter((image) => {
      const bounds = image.getBoundingClientRect()
      return bounds.width && bounds.height && bounds.bottom > 0 && bounds.top < innerHeight && bounds.right > 0 && bounds.left < innerWidth
    }).map((image) => image.decode().catch(() => {})))
    await new Promise((resolve) => requestAnimationFrame(resolve))
  })(), new Promise(resolve => setTimeout(resolve, 1500))]))
  return page.screenshot({ animations: "disabled", ...options })
}

async function freePort(except) {
  const listener = net.createServer()
  listener.listen(0, "127.0.0.1")
  await once(listener, "listening")
  const port = listener.address().port
  await new Promise((resolve) => listener.close(resolve))
  return port === except ? freePort(except) : port
}

async function freshServer(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "ember-workspace-browser-"))
  const port = await freePort()
  const targetPort = await freePort(port)
  const origin = `http://127.0.0.1:${port}`
  const lifecycle = { closing: false }
  const env = { ...process.env }
  // Each test owns its complete installation; never inherit the user's running app or data.
  for (const key of Object.keys(env)) {
    if (/^(EMBER_|MATCHBOX_|CAMPFIRE_|THRUSTER_|VAPID_)/.test(key)) delete env[key]
  }
  Object.assign(env, {
    SECRET_KEY_BASE: "workspace-disposable-browser-test-secret".repeat(4),
    EMBER_STORAGE_PATH: dir,
    EMBER_DATABASE_PATH: path.join(dir, "db/production.sqlite3"),
    EMBER_FILES_PATH: path.join(dir, "files"),
    EMBER_BACKUPS_PATH: path.join(dir, "backups"),
    EMBER_LOG: "error",
    RAILS_ENV: "production",
    DISABLE_SSL: "1",
    THRUSTER_HTTP_PORT: String(port),
    THRUSTER_TARGET_PORT: String(targetPort),
    THRUSTER_TARGET_BIND: "127.0.0.1",
    THRUSTER_TLS_DOMAIN: "",
    THRUSTER_STORAGE_PATH: path.join(dir, "thruster"),
    THRUSTER_LOG_REQUESTS: "false",
    TOKIO_WORKER_THREADS: "2",
    RAILS_MAX_THREADS: "2",
    JOB_CONCURRENCY: "1",
  })
  const child = spawn(binary, [], { cwd: dir, env, stdio: ["ignore", "pipe", "pipe"] })
  let log = ""
  let spawnError
  child.on("error", (error) => { spawnError = error })
  for (const stream of [child.stdout, child.stderr]) stream.on("data", (data) => { log += data })
  t.after(async () => {
    lifecycle.closing = true
    if (child.exitCode === null && !spawnError) {
      const exited = once(child, "exit")
      child.kill("SIGTERM")
      const timer = setTimeout(() => child.kill("SIGKILL"), 5000)
      await exited
      clearTimeout(timer)
    }
    if (log) await fs.writeFile(path.join(artifacts, `workspace-${port}.server.log`), log)
    await fs.rm(dir, { recursive: true, force: true })
  })
  const deadline = Date.now() + 20_000
  while (Date.now() < deadline) {
    if (spawnError) throw spawnError
    if (child.exitCode !== null) throw new Error(`Ember exited: ${log}`)
    try {
      const response = await fetch(`${origin}/first_run`, { signal: AbortSignal.timeout(1000) })
      if (response.ok) return { origin, dir, lifecycle }
    } catch { /* The listening sockets are not ready yet. */ }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`Ember did not become ready: ${log}`)
}

async function newPage(t, server, options = {}) {
  const context = await browser.newContext({ baseURL: server.origin, colorScheme: "light", ...options })
  const page = await context.newPage()
  const number = ++pageNumber
  const requests = []
  page.on("response", (response) => {
    const request = response.request()
    if (request.method() !== "GET") requests.push(`${request.method()} ${new URL(response.url()).pathname} ${response.status()}`)
  })
  t.after(async () => {
    if (!page.isClosed()) {
      const label = `${t.name.replace(/[^a-z0-9]+/gi, "-").toLowerCase()}-${number}`
      await capture(page, { path: path.join(artifacts, `workspace-last-${label}.png`), fullPage: true }).catch(() => {})
      await fs.writeFile(path.join(artifacts, `workspace-last-${label}.html`), await page.content()).catch(() => {})
      await fs.writeFile(path.join(artifacts, `workspace-last-${label}.requests.txt`), requests.join("\n"))
    }
    await context.close()
  })
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on("pageerror", (error) => {
    if (!server.lifecycle.closing) errors.push(`${error.name}: ${error.message}\n${error.stack || ""}`)
  })
  page.on("console", (message) => {
    if (!server.lifecycle.closing && message.type() === "error" && /(?:error.*controller|error.*action|stimulus)/i.test(message.text())) errors.push(message.text())
  })
  t.after(() => assert.deepEqual(errors, [], "browser JavaScript errors"))
  return { page, context }
}

async function setUp(t, options) {
  const server = await freshServer(t)
  const { page, context } = await newPage(t, server, options)
  await page.goto("/first_run")
  await page.locator("#user_name").fill("Ada Lovelace")
  await page.locator("#user_email_address").fill("ada@example.test")
  await page.locator("#user_password").fill(password)
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  await page.locator("#composer").waitFor()
  // On mobile the loaded conversation list lives inside the closed drawer.
  await page.locator("#shared_rooms a").first().waitFor({ state: "attached" })
  const roomPath = new URL(page.url()).pathname
  const joinURL = await page.locator("#invite_url").inputValue()
  return { server, page, context, roomPath, joinURL }
}

async function noOverflow(page, label) {
  const sizes = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    document: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
  }))
  assert.ok(sizes.document <= sizes.viewport + 1, `${label}: document overflows ${JSON.stringify(sizes)}`)
  assert.ok(sizes.body <= sizes.viewport + 1, `${label}: body overflows ${JSON.stringify(sizes)}`)
  const clippedFields = await page.locator('#main-content input:not([type="hidden"]):not([type="file"]):not([type="checkbox"]), #main-content textarea').evaluateAll((fields) => fields.flatMap((field) => {
    const bounds = field.getBoundingClientRect()
    if (!bounds.width || !bounds.height || getComputedStyle(field).visibility === "hidden") return []
    return bounds.left < -1 || bounds.right > document.documentElement.clientWidth + 1 ? [field.id || field.name] : []
  }))
  assert.deepEqual(clippedFields, [], `${label}: form fields must fit, not merely be clipped`)
}

async function shell(page) {
  await page.locator("body.mb-authenticated nav.mb-rail").waitFor({ state: "attached" })
  assert.equal(await page.locator("aside#sidebar.mb-sidebar").count(), 1)
  assert.equal(await page.locator("#main-content.mb-main").count(), 1)
  const sheets = await page.locator('link[rel="stylesheet"]').evaluateAll((links) => links.map((link) => link.href))
  assert.equal(sheets.filter((href) => href.includes("zz-ember")).length, 1, "workspace stylesheet loads once")
}

async function submitMessage(page, roomPath, text) {
  await page.locator(`#composer[action="${roomPath}/messages"]`).waitFor()
  const editable = page.locator('#composer lexxy-editor [contenteditable="true"]')
  await editable.click()
  await editable.pressSequentially(text)
  await page.waitForFunction((text) => document.querySelector('#composer lexxy-editor')?.value.includes(text), text)
  const posted = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === `${roomPath}/messages`)
  await page.getByRole("button", { name: "Send Message", exact: true }).click()
  assert.equal((await posted).status(), 200)
  const message = page.locator(".message[data-message-id]").filter({ hasText: text })
  await message.waitFor()
  return message
}

test("workspace navigation, global search, and room creation use the real application", { timeout: 90_000 }, async (t) => {
  const { page, context, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1440, height: 1000 } })
  await shell(page)
  const boxes = await page.evaluate(() => ["nav.mb-rail", "aside#sidebar", "#main-content"].map((selector) => {
    const node = document.querySelector(selector)
    const { x, width } = node.getBoundingClientRect()
    return { x, width }
  }))
  assert.ok(boxes[0].x < boxes[1].x && boxes[1].x < boxes[2].x, "rail, conversations, and content are arranged left to right")
  await page.locator("nav.mb-rail [data-ember-activity]").click()
  assert.equal(await page.locator("[data-ember-activity]").getAttribute("aria-pressed"), "true")
  await page.locator("[data-ember-room-empty]").getByText("You’re all caught up. No unread conversations.", { exact: true }).waitFor()
  await page.locator("nav.mb-rail [data-ember-activity]").click()
  assert.equal(await page.locator('#sidebar a[href="/rooms/opens/new"]').count(), 0)
  await page.locator('nav.mb-rail').getByRole('link', { name: 'Admin', exact: true }).click()
  await page.locator('#channel-settings').getByRole('link', { name: 'New room', exact: true }).click()
  await page.locator("#room_name").fill("Design")
  await page.getByRole("button", { name: "Save", exact: true }).click()
  await page.locator("#composer").waitFor()
  const designPath = new URL(page.url()).pathname
  assert.notEqual(designPath, roomPath)
  assert.equal(await page.locator("[data-ember-room-filter]").count(), 0)
  assert.equal(await page.locator('.mb-workspace-heading a[href="/rooms/opens/new"]').count(), 0)
  assert.equal(await page.getByRole('heading', { name: 'Rooms', exact: true }).count(), 0)
  const railLabels = await page.locator('.mb-rail__link span:first-of-type').allTextContents()
  assert.ok(railLabels.indexOf('Search') === railLabels.indexOf('Activity') + 1)
  await page.locator('nav.mb-rail').getByRole('link', { name: 'Search', exact: true }).click()
  await page.locator('form[action="/searches"] input[name="q"]').waitFor()
  assert.equal(await page.locator('nav.mb-rail a[href="/searches"]').getAttribute('aria-current'), 'page')
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 900 })
    await page.getByRole('searchbox', { name: 'search', exact: true }).focus()
    const search = page.locator('#footer .composer__input')
    const exit = page.getByRole('link', { name: 'Exit search', exact: true })
    const [fieldBox, exitBox] = await Promise.all([search.boundingBox(), exit.boundingBox()])
    assert.ok(Math.abs(fieldBox.y + fieldBox.height / 2 - exitBox.y - exitBox.height / 2) < 2, 'search and exit button are vertically centered')
    assert.equal(await search.evaluate(node => getComputedStyle(node).boxShadow), 'none', 'search has no thick outer focus ring')
    await noOverflow(page, `${width}px search`)
    await capture(page, { path: path.join(artifacts, `workspace-search-${width}.png`) })
  }
  await page.setViewportSize({ width: 1440, height: 1000 })
  await page.locator(`#shared_rooms a[href="${roomPath}"]`).click()
  await page.waitForURL(`**${roomPath}`)
  await page.waitForFunction((path) => document.querySelector(`#shared_rooms a[href="${path}"]`)?.getAttribute("aria-current") === "page", roomPath)
  await page.getByText("Welcome to Ember", { exact: true }).waitFor()

  const { context: member } = await newPage(t, server)
  const joined = await member.request.post(joinURL, {
    headers: sameOrigin,
    multipart: { "user[name]": "Maya Chen", "user[email_address]": "maya@example.test", "user[password]": password },
  })
  assert.equal(joined.status(), 200)
  await page.locator('#sidebar a[href="/rooms/directs/new"]').first().click()
  await page.getByRole("dialog", { name: "New direct message" }).waitFor()
  assert.equal(new URL(page.url()).pathname, roomPath)
  await page.getByRole("combobox", { name: "Search people" }).fill("@Maya")
  await page.locator("[role=option]").filter({ hasText: "Maya Chen" }).click()
  await page.waitForFunction(() => document.querySelectorAll('.mb-dm-dialog input[name="user_ids[]"]').length > 0)
  const directForm = await page.getByRole("button", { name: "Start conversation", exact: true }).evaluate((button) => ({
    form: button.form?.action,
    valid: button.form?.checkValidity(),
    values: [...(button.form?.elements || [])].map((element) => ({ name: element.name, value: element.value, valid: element.validity?.valid, disabled: element.disabled })),
  }))
  assert.ok(directForm.form && directForm.valid, `direct message form must be valid: ${JSON.stringify(directForm)}`)
  await page.getByRole("button", { name: "Start conversation", exact: true }).click()
  await page.waitForURL((url) => url.pathname !== roomPath && /^\/rooms\/\d+$/.test(url.pathname))
  await page.locator("#composer").waitFor()
  const directPath = new URL(page.url()).pathname
  assert.notEqual(directPath, roomPath)
  await page.waitForLoadState("networkidle")
  await submitMessage(page, directPath, "A private conversation that actually works.")
  await page.locator(`#shared_rooms a[href="${roomPath}"]`).click()
  await page.waitForURL(`**${roomPath}`)
  await page.locator(`#direct_rooms a[href="${directPath}"]`).click()
  await page.waitForURL(`**${directPath}`)
  await page.getByText("A private conversation that actually works.", { exact: true }).waitFor()

  await page.locator('nav.mb-rail a[href="/account/edit"]').click()
  await page.locator("#account_name").fill("Ember Studio")
  const accountForm = page.locator("form").filter({ has: page.locator("#account_name") })
  const accountSaved = page.waitForResponse((response) => response.request().method() === "POST" && /^\/account(?:\.|$)/.test(new URL(response.url()).pathname))
  await accountForm.getByRole("button", { name: "Save changes", exact: true }).click()
  assert.ok([302, 303].includes((await accountSaved).status()))
  await page.locator("#account_name").waitFor()
  await page.reload()
  assert.equal(await page.locator("#account_name").inputValue(), "Ember Studio")
  assert.equal(await page.getByRole("checkbox", { name: "Hide translation buttons", exact: true }).isChecked(), true)
  await shell(page)
  await capture(page, { path: path.join(artifacts, "workspace-settings.png"), fullPage: true })

  await page.locator('nav.mb-rail a[href="/users/me/profile"]').click()
  await page.locator("#user_bio").fill("Designing a calmer place to talk.")
  const profileSaved = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === "/users/me/profile")
  await page.getByRole("button", { name: "Save changes", exact: true }).click()
  assert.ok([302, 303].includes((await profileSaved).status()))
  await page.locator("#user_bio").waitFor()
  await page.reload()
  assert.equal(await page.locator("#user_bio").inputValue(), "Designing a calmer place to talk.")
  await shell(page)
  await noOverflow(page, "desktop profile")
  assert.equal((await context.request.get(designPath)).status(), 200)
})

test("uploads show progress and processing, reject oversized files, and recover from failures", { timeout: 90_000 }, async t => {
  const { page, roomPath } = await setUp(t, { viewport: { width: 1200, height: 900 } })
  // Hold uploads at the network boundary to inspect progress without depending on loopback speed.
  await page.evaluate(() => {
    const NativeRequest = window.XMLHttpRequest
    window.uploadRequests = []
    window.XMLHttpRequest = class extends NativeRequest {
      send(body) { window.uploadRequests.push(this); super.send(body) }
    }
  })
  let release
  const held = new Promise(resolve => { release = resolve })
  await page.route(`**${roomPath}/messages`, async route => {
    await held
    await route.continue()
  })
  await page.locator('#composer input[type="file"]').setInputFiles({ name: 'project-video.mov', mimeType: 'video/quicktime', buffer: await fs.readFile(path.join(repo, 'reference/test/fixtures/files/alpha-centuri.mov')) })
  await page.getByRole('button', { name: 'Send Message', exact: true }).click()
  const card = page.locator('.mb-upload').filter({ hasText: 'project-video.mov' })
  await card.waitFor()
  await page.waitForFunction(() => window.uploadRequests.length === 1)
  await page.evaluate(() => window.uploadRequests[0].upload.dispatchEvent(new ProgressEvent('progress', { lengthComputable: true, loaded: 420, total: 1000 })))
  await page.waitForFunction(() => document.querySelector('.mb-upload progress')?.value === 42)
  assert.match(await card.textContent(), /Uploading · 42%/)
  await noOverflow(page, 'desktop upload progress')
  await capture(page, { path: path.join(artifacts, 'workspace-upload-progress.png'), fullPage: true })
  await page.setViewportSize({ width: 390, height: 844 })
  await page.emulateMedia({ colorScheme: 'dark' })
  await noOverflow(page, 'mobile upload progress')
  await capture(page, { path: path.join(artifacts, 'workspace-upload-progress-mobile.png'), fullPage: true })
  await page.evaluate(() => window.uploadRequests[0].upload.dispatchEvent(new ProgressEvent('progress', { lengthComputable: true, loaded: 1000, total: 1000 })))
  await card.getByText('Processing…', { exact: true }).waitFor()
  const response = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === `${roomPath}/messages`)
  release()
  assert.equal((await response).status(), 200)
  await page.locator('.message[data-message-id] video[controls]').waitFor()
  assert.equal(await card.count(), 0)
  await page.unroute(`**${roomPath}/messages`)

  // File metadata is enough to reject it; never allocate or transmit a huge browser fixture.
  await page.evaluate(() => {
    const file = new File(['x'], 'too-large.mp4', { type: 'video/mp4' })
    Object.defineProperty(file, 'size', { value: 250_000_001 })
    window.dispatchEvent(new CustomEvent('drop-target:drop', { detail: { files: [file] } }))
  })
  await page.locator('#composer [role="alert"]').getByText(/250 MB or smaller/).waitFor()
  assert.equal(await page.locator('#composer .composer__file').count(), 0)
  assert.equal(await page.evaluate(() => window.uploadRequests.length), 1)

  let requests = 0
  await page.route(`**${roomPath}/messages`, route => {
    if (++requests === 1) return route.fulfill({ status: 413, body: '' })
    return route.continue()
  })
  await page.locator('#composer input[type="file"]').setInputFiles([
    { name: 'a-rejected.txt', mimeType: 'text/plain', buffer: Buffer.from('refused') },
    { name: 'b-successful.txt', mimeType: 'text/plain', buffer: Buffer.from('accepted') },
  ])
  await page.getByRole('button', { name: 'Send Message', exact: true }).click()
  await page.locator('.mb-upload--failed').getByText('Files must be 250 MB or smaller.', { exact: true }).waitFor()
  await page.locator('.message[data-message-id]').filter({ hasText: 'b-successful.txt' }).waitFor()
  assert.equal(requests, 2)
})

test("the server rejects files over 250 MB even without browser validation", { timeout: 90_000 }, async t => {
  const { page, context, server, roomPath } = await setUp(t)
  const boundary = 'EmberUploadBoundary'
  const chunk = Buffer.alloc(1_000_000, 'x')
  async function* body() {
    yield `--${boundary}\r\nContent-Disposition: form-data; name="message[attachment]"; filename="too-large.bin"\r\nContent-Type: application/octet-stream\r\n\r\n`
    for (let i = 0; i < 250; i++) yield chunk
    yield `x\r\n--${boundary}--\r\n`
  }
  const cookies = (await context.cookies()).map(cookie => `${cookie.name}=${cookie.value}`).join('; ')
  const response = await fetch(`${server.origin}${roomPath}/messages`, {
    method: 'POST', body: body(), duplex: 'half',
    headers: { ...sameOrigin, Cookie: cookies, 'Content-Type': `multipart/form-data; boundary=${boundary}`, Accept: 'text/vnd.turbo-stream.html' },
  })
  assert.equal(response.status, 413)
  await response.arrayBuffer()
  await page.reload()
  assert.equal(await page.locator('.message[data-message-id]').filter({ hasText: 'too-large.bin' }).count(), 0)
})

test("chat sends, edits, reacts, uploads, and searches without losing its controllers", { timeout: 90_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 1440, height: 1000 } })
  const firstMessage = await submitMessage(page, roomPath, "A quieter space for our team.")
  await capture(page, { path: path.join(artifacts, "workspace-first-message.png"), fullPage: true })
  await fs.writeFile(path.join(artifacts, "workspace-first-message.json"), JSON.stringify(await firstMessage.evaluate((node) => ({
    classes: node.className,
    author: node.querySelector(".message__author")?.textContent,
    authorDisplay: node.querySelector(".message__author") && getComputedStyle(node.querySelector(".message__author")).display,
    authorVisibility: node.querySelector(".message__author") && getComputedStyle(node.querySelector(".message__author")).visibility,
    meta: node.querySelector(".message__meta")?.innerHTML,
  })), null, 2))
  await submitMessage(page, roomPath, "Every detail has a place.")
  await page.reload()
  const messageId = await page.locator(".message[data-message-id]").filter({ hasText: "Every detail has a place." }).getAttribute("data-message-id")
  const message = page.locator(`.message[data-message-id="${messageId}"]`)
  await message.hover()
  await message.locator("summary").filter({ hasText: "Message options" }).click()
  await message.getByRole("link", { name: "Edit", exact: true }).click()
  await message.locator('lexxy-editor [contenteditable="true"]').fill("Every detail has a place in Ember.")
  const saveForm = await message.getByRole("button", { name: "Save changes", exact: true }).evaluate((button) => ({
    form: button.form?.id,
    valid: button.form?.checkValidity(),
    invalid: [...(button.form?.elements || [])].filter((element) => !element.validity?.valid).map((element) => ({ name: element.name, validation: element.validationMessage })),
  }))
  assert.ok(saveForm.form && saveForm.valid, `message editor must have a valid connected form: ${JSON.stringify(saveForm)}`)
  const saved = page.waitForResponse((response) => response.request().method() === "POST" && /\/messages\/\d+$/.test(new URL(response.url()).pathname))
  await message.getByRole("button", { name: "Save changes", exact: true }).click()
  assert.ok([302, 303].includes((await saved).status()))
  const edited = page.locator(".message[data-message-id]").filter({ hasText: "Every detail has a place in Ember." })
  await edited.hover()
  await edited.locator("summary").filter({ hasText: "Message options" }).click()
  const boosted = page.waitForResponse((response) => response.request().method() === "POST" && /\/boosts$/.test(new URL(response.url()).pathname))
  await edited.getByRole("button", { name: "Thumbs up", exact: true }).click()
  assert.ok([302, 303].includes((await boosted).status()))
  await edited.locator(".boost").first().waitFor()

  const uploaded = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === `${roomPath}/messages`)
  await page.locator('#composer input[type="file"]').setInputFiles({ name: "workspace-notes.txt", mimeType: "text/plain", buffer: Buffer.from("A working attachment in the redesigned workspace.\n") })
  await page.getByRole("button", { name: "Send Message", exact: true }).click()
  assert.equal((await uploaded).status(), 200)
  const attachment = page.locator(".message[data-message-id]").filter({ hasText: "workspace-notes.txt" })
  await attachment.waitFor()
  const download = attachment.locator('a[aria-label="Download"]')
  const downloaded = await context.request.get(await download.getAttribute("href"))
  assert.equal(await downloaded.text(), "A working attachment in the redesigned workspace.\n")
  const imageUploaded = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === `${roomPath}/messages`)
  await page.locator('#composer input[type="file"]').setInputFiles(path.join(repo, "reference/test/fixtures/files/earth.png"))
  await page.getByRole("button", { name: "Send Message", exact: true }).click()
  assert.equal((await imageUploaded).status(), 200)
  await page.locator('.message img.message__attachment').first().waitFor()
  await noOverflow(page, "desktop conversation")
  await capture(page, { path: path.join(artifacts, "workspace-desktop.png"), fullPage: true })

  await page.locator('#nav a[href="/searches"]').first().click()
  await page.locator("#q").fill("detail")
  await page.locator('form[action="/searches"]').getByRole("button", { name: "Search", exact: true }).click()
  await page.locator("#search-results").getByText("Every detail has a place in Ember.", { exact: true }).waitFor()
  await shell(page)
  await noOverflow(page, "desktop search")
  await capture(page, { path: path.join(artifacts, "workspace-search.png"), fullPage: true })
  await page.goBack()
  await page.goBack()
  await page.locator("#composer").waitFor()
  await submitMessage(page, roomPath, "Back navigation keeps the composer working.")
  assert.equal(await page.locator("nav.mb-rail").count(), 1)
  assert.equal(await page.locator("[data-ember-room-filter]").count(), 0)
})

test("mobile and tablet navigation keep the conversation and settings usable", { timeout: 90_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true })
  await submitMessage(page, roomPath, "A very long link remains inside the conversation: https://example.test/" + "a".repeat(180))
  const image = await context.request.post(`${roomPath}/messages`, {
    headers: { ...sameOrigin, Accept: "text/vnd.turbo-stream.html" },
    multipart: { "message[attachment]": { name: "earth.png", mimeType: "image/png", buffer: await fs.readFile(path.join(repo, "reference/test/fixtures/files/earth.png")) } },
  })
  assert.equal(image.status(), 200)
  for (const width of [320, 390, 768]) {
    await page.setViewportSize({ width, height: 844 })
    await page.goto(roomPath)
    await shell(page)
    await noOverflow(page, `${width}px conversation`)
    const imageBounds = await page.locator('.message img.message__attachment').first().boundingBox()
    assert.ok(imageBounds.x >= -1 && imageBounds.x + imageBounds.width <= width + 1, `${width}px uploaded image fits the conversation`)
    if (width <= 900) {
      const toggle = page.locator("[data-ember-sidebar-toggle]").filter({ visible: true }).first()
      await toggle.waitFor()
      assert.equal(await toggle.getAttribute("aria-expanded"), "false")
      await toggle.click()
      await page.waitForFunction(() => document.querySelector('[data-ember-sidebar-toggle][aria-expanded="true"]'))
      await page.waitForFunction(() => document.querySelector("#sidebar").getBoundingClientRect().x >= 0)
      const sidebar = page.locator("#sidebar")
      const bounds = await sidebar.boundingBox()
      assert.ok(bounds.x >= -1 && bounds.x + bounds.width <= width + 1, `${width}px drawer fits the viewport`)
      await noOverflow(page, `${width}px drawer`)
      for (const key of ["Tab", "Tab", "Tab", "Tab", "Tab", "Tab", "Tab", "Shift+Tab", "Shift+Tab"]) {
        await page.keyboard.press(key)
        assert.equal(await page.evaluate(() => !!document.activeElement.closest(".mb-rail, .mb-sidebar")), true, "keyboard focus stays in the open drawer")
      }
      await page.keyboard.press("Escape")
      assert.equal(await toggle.getAttribute("aria-expanded"), "false")
      assert.equal(await toggle.evaluate((node) => node === document.activeElement), true, "Escape restores focus to the menu toggle")
      await toggle.click()
      await page.locator(".mb-sidebar-close[data-ember-sidebar-close]").click()
      assert.equal(await toggle.getAttribute("aria-expanded"), "false")
      await toggle.click()
      await page.locator(".mb-sidebar-backdrop[data-ember-sidebar-close]").click({ position: { x: width - 4, y: 420 } })
      assert.equal(await toggle.getAttribute("aria-expanded"), "false", "backdrop closes the drawer")
      await toggle.click()
      await page.locator(`#shared_rooms a[href="${roomPath}"]`).click()
      await page.waitForFunction(() => !document.querySelector('[data-ember-sidebar-toggle][aria-expanded="true"]'))
      await page.waitForFunction(() => getComputedStyle(document.querySelector("#sidebar")).visibility === "hidden")
    } else {
      assert.equal(await page.locator("#sidebar").isVisible(), true, "tablet conversations remain visible alongside the chat")
    }
    await page.locator("#composer").waitFor()
    await capture(page, { path: path.join(artifacts, `workspace-mobile-${width}.png`), fullPage: true })

    for (const pathname of ["/account/edit", "/users/me/profile", "/rooms/opens/new", "/account/bots", "/account/bots/new", "/searches"]) {
      await page.goto(pathname)
      await shell(page)
      await noOverflow(page, `${width}px ${pathname}`)
      const main = page.locator("#main-content")
      assert.ok((await main.boundingBox()).width <= width + 1)
    }
  }
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto("/users/me/profile")
  await page.locator("#user_bio").fill("An update made from a phone.")
  const profileSaved = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === "/users/me/profile")
  await page.getByRole("button", { name: "Save changes", exact: true }).click()
  assert.ok([302, 303].includes((await profileSaved).status()))
  await page.locator("#user_bio").waitFor()
  await page.reload()
  assert.equal(await page.locator("#user_bio").inputValue(), "An update made from a phone.")
  await capture(page, { path: path.join(artifacts, "workspace-mobile-settings.png"), fullPage: true })
  await page.setViewportSize({ width: 390, height: 420 })
  await page.goto(roomPath)
  const composer = await page.locator("#composer").boundingBox()
  assert.ok(composer.y >= 0 && composer.y + composer.height <= 421, "composer remains reachable in a short mobile viewport")
  await noOverflow(page, "short mobile conversation")
})

for (const nativePopover of [true, false]) {
test(`workspace appearance persists, follows the system, and remains accessible on mobile (${nativePopover ? "native" : "fallback"})`, { timeout: 60_000 }, async (t) => {
  const { server, page, context, roomPath, joinURL } = await setUp(t, { viewport: { width: 1280, height: 900 } })
  if (!nativePopover) await context.addInitScript(() => {
    delete HTMLElement.prototype.showPopover
    delete HTMLElement.prototype.hidePopover
    delete HTMLElement.prototype.togglePopover
    const query = Document.prototype.querySelector
    Document.prototype.querySelector = function(selector) {
      if (selector.includes(':popover-open')) throw new SyntaxError('Unsupported popover selector')
      return query.call(this, selector)
    }
  })
  await page.goto(roomPath)
  const surface = () => page.locator("#main-content").evaluate((node) => getComputedStyle(node).backgroundColor)
  const light = await surface()
  await page.emulateMedia({ colorScheme: "dark" })
  await page.waitForFunction((previous) => getComputedStyle(document.querySelector("#main-content")).backgroundColor !== previous, light)
  const dark = await surface()
  assert.notEqual(dark, light)
  await capture(page, { path: path.join(artifacts, "workspace-dark.png"), fullPage: true })
  await page.emulateMedia({ colorScheme: "light" })
  await page.waitForFunction((expected) => getComputedStyle(document.querySelector("#main-content")).backgroundColor === expected, light)

  const trigger = page.locator('.mb-appearance-toggle')
  const menu = page.getByRole('dialog', { name: 'Appearance', exact: true })
  const tabOutOfFallback = async () => {
    if (nativePopover) return
    for (const [key, destination] of [
      ['Tab', page.locator('.mb-rail__profile')],
      ['Shift+Tab', page.locator('.mb-rail').getByRole('link', { name: 'Admin', exact: true })],
    ]) {
      await trigger.click()
      await page.keyboard.press(key)
      assert.equal(await menu.isVisible(), false, `${key} closes the fallback appearance menu`)
      assert.equal(await destination.evaluate(node => node === document.activeElement), true, `${key} continues through workspace controls`)
    }
  }
  const select = async mode => {
    if (!await menu.isVisible()) await trigger.click()
    await menu.getByText(mode, { exact: true }).click()
    assert.equal(await menu.getByRole('radio', { name: mode, exact: true }).isChecked(), true)
  }
  const [toggleBox, avatarBox] = await Promise.all([trigger.boundingBox(), page.locator('.mb-rail__profile').boundingBox()])
  assert.ok(toggleBox.y + toggleBox.height < avatarBox.y, 'appearance is directly above the profile avatar')
  assert.equal(toggleBox.width, toggleBox.height)
  await select('Dark')
  assert.equal(await surface(), dark, 'explicit dark overrides a light system')
  assert.equal(await page.locator('.mb-composer-send img').evaluate(node => getComputedStyle(node).filter), 'invert(0)', 'send icon remains visible on its reversed button')
  await capture(page, { path: path.join(artifacts, 'workspace-appearance-dark.png') })
  await page.keyboard.press('Escape')
  assert.equal(await menu.isVisible(), false)
  assert.equal(await trigger.evaluate(node => node === document.activeElement), true)
  await tabOutOfFallback()
  await page.locator('.mb-rail__profile').click()
  await page.locator('#user_name').waitFor()
  assert.equal(await surface(), dark, 'preference survives Turbo navigation')
  await page.reload()
  assert.equal(await surface(), dark, 'preference survives a full reload')
  await page.emulateMedia({ colorScheme: 'dark' })
  await select('Light')
  assert.equal(await surface(), light, 'explicit light overrides a dark system')
  assert.equal(await page.locator('#nav .btn img').first().evaluate(node => getComputedStyle(node).filter), 'invert(0)', 'inherited icons use the selected palette')

  const sibling = await context.newPage()
  await sibling.goto(roomPath)
  await select('System')
  assert.equal(await surface(), dark)
  await sibling.waitForFunction(() => document.documentElement.dataset.appearance === 'system')
  await sibling.close()
  await page.emulateMedia({ colorScheme: 'light' })
  await page.waitForFunction(expected => getComputedStyle(document.querySelector('#main-content')).backgroundColor === expected, light)
  await menu.getByRole('radio', { name: 'System', exact: true }).focus()
  await page.keyboard.press('ArrowUp')
  assert.equal(await menu.getByRole('radio', { name: 'Dark', exact: true }).isChecked(), true)
  assert.equal(await surface(), dark, 'native radio keys change the appearance')
  await page.keyboard.press('Escape')

  await page.setViewportSize({ width: 320, height: 740 })
  await page.getByRole('button', { name: 'Open conversations', exact: true }).click()
  await trigger.click()
  await menu.waitFor()
  const popover = await menu.boundingBox()
  assert.ok(popover.x >= 0 && popover.x + popover.width <= 320)
  await capture(page, { path: path.join(artifacts, 'workspace-appearance-mobile.png') })
  await page.keyboard.press('Escape')
  assert.equal(await menu.isVisible(), false)
  assert.equal(await page.locator('body').evaluate(node => node.classList.contains('mb-sidebar-open')), true, 'Escape closes appearance before the mobile drawer')
  await tabOutOfFallback()
  await page.keyboard.press('Escape')

  await page.addInitScript(() => {
    Storage.prototype.getItem = () => { throw new Error('Storage unavailable') }
    Storage.prototype.setItem = () => { throw new Error('Storage unavailable') }
  })
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.reload()
  assert.equal(await surface(), light)
  await select('Dark')
  assert.equal(await surface(), dark, 'appearance still works when storage is blocked')

  const { page: guest } = await newPage(t, server, { viewport: { width: 320, height: 740 } })
  for (const pathname of ["/session/new", joinURL]) {
    await guest.goto(pathname)
    await noOverflow(guest, `320px ${new URL(guest.url()).pathname}`)
    assert.equal(await guest.locator("body.mb-authenticated").count(), 0, "signed-out pages have no workspace navigation")
    await guest.locator('input[type="email"]').fill("ada@example.test")
    await guest.locator('input[type="password"]').fill(password)
  }
  await guest.goto("/session/new")
  await guest.locator('input[type="email"]').fill("ada@example.test")
  await guest.locator('input[type="password"]').fill(password)
  await capture(guest, { path: path.join(artifacts, "workspace-mobile-sign-in.png"), fullPage: true })
  await guest.locator('form').filter({ has: guest.locator('input[type="password"]') }).locator('button[type="submit"]').click()
  await guest.locator("#composer").waitFor()
  await shell(guest)
  await noOverflow(guest, "320px signed-in conversation")
})

}

test("new rooms return to settings and profile saves show one centered confirmation", { timeout: 60_000 }, async (t) => {
  const { page } = await setUp(t, { viewport: { width: 1311, height: 900 } })
  await page.locator('.mb-rail').getByRole('link', { name: 'Admin', exact: true }).click()
  await page.locator('#channel-settings').getByRole('link', { name: 'New room', exact: true }).click()
  await page.locator('#room_name').waitFor()
  assert.equal(await page.getByRole('link', { name: 'Go Back', exact: true }).getAttribute('href'), '/account/edit')
  await page.getByRole('link', { name: 'Go Back', exact: true }).click()
  await page.waitForURL('**/account/edit')
  await page.goto('/rooms/closeds/new')
  assert.equal(await page.getByRole('link', { name: 'Go Back', exact: true }).getAttribute('href'), '/account/edit')
  await page.locator('#room_name').fill('Unsaved room')
  await page.getByRole('link', { name: 'Go Back', exact: true }).click()
  await page.getByRole('dialog', { name: 'Unsaved Changes', exact: true }).getByRole('button', { name: 'Discard', exact: true }).click()
  await page.waitForURL('**/account/edit')
  await page.locator('.mb-rail__profile').click()
  await page.locator('#user_name').waitFor()
  assert.equal(await page.locator('#user_name').getAttribute('autofocus'), null)
  assert.equal(await page.locator('#user_name').evaluate(node => node === document.activeElement), false, 'opening the profile does not focus its name')

  for (const width of [1311, 1000, 390]) {
    await page.setViewportSize({ width, height: 900 })
    await page.locator('#user_bio').fill(`Saved at ${width}px`)
    const saved = page.waitForResponse(response => response.request().method() === 'GET' && new URL(response.url()).pathname === '/users/me/profile')
    await page.getByRole('button', { name: 'Save changes', exact: true }).click()
    await saved
    await page.waitForFunction(value => document.querySelector('#user_bio')?.defaultValue === value, `Saved at ${width}px`)
    const flash = page.locator('.flash__inner')
    await flash.waitFor()
    await flash.evaluate(node => { node.style.animation = 'none' })
    assert.equal(await flash.locator('img').count(), 1)
    assert.equal(await flash.locator('span').innerText(), 'Changes saved')
    assert.equal(await flash.locator('span').getAttribute('class'), 'for-screen-reader')
    const [notice, pane] = await Promise.all([flash.boundingBox(), page.locator('#main-content').boundingBox()])
    assert.ok(Math.abs(notice.x + notice.width / 2 - pane.x - pane.width / 2) < 1, 'confirmation centers within the content pane')
    assert.ok(notice.width >= 48 && notice.height >= 48)
    await capture(page, { path: path.join(artifacts, `workspace-save-confirmation-${width}.png`) })
  }
  await page.setViewportSize({ width: 1311, height: 900 })
  await page.goto('/account/custom_styles/edit')
  await page.locator('#account_custom_styles').fill('/* Saved custom styles */')
  await page.getByRole('button', { name: 'Save changes', exact: true }).click()
  await page.waitForFunction(() => document.querySelector('#account_custom_styles')?.defaultValue === '/* Saved custom styles */')
  await page.locator('.flash__inner').waitFor()
  assert.equal(await page.locator('.flash__inner img').count(), 1, 'custom styles use the same single-icon confirmation')
  assert.equal(await page.locator('.flash__inner span').innerText(), 'Changes saved')
})

test("reused client message IDs cannot replace another member's persisted message", { timeout: 60_000 }, async (t) => {
  const { page, context, roomPath, joinURL, server } = await setUp(t)
  const { page: memberPage, context: member } = await newPage(t, server)
  const joined = await member.request.post(joinURL, {
    headers: sameOrigin,
    multipart: { "user[name]": "Maya Chen", "user[email_address]": "maya@example.test", "user[password]": password },
  })
  assert.equal(joined.status(), 200)
  await memberPage.goto(roomPath)
  await memberPage.locator("#composer").waitFor()
  await Promise.all([page.waitForLoadState("networkidle"), memberPage.waitForLoadState("networkidle")])

  const correlation = "reused-correlation"
  const create = async (session, text) => {
    const response = await session.request.post(`${roomPath}/messages`, {
      headers: { ...sameOrigin, Accept: "text/vnd.turbo-stream.html" },
      form: { "message[body]": text, "message[client_message_id]": correlation },
    })
    assert.equal(response.status(), 200)
    return response.text()
  }
  const victimText = "Ada's original message must survive"
  const victimHtml = await create(context, victimText)
  const victimId = victimHtml.match(/data-message-id="(\d+)"/)[1]
  const victimSelector = `#message_${victimId}`
  // Apply HTTP and Cable acknowledgments to the same live DOM; repeats must stay harmless.
  for (const observer of [page, memberPage]) {
    await observer.evaluate(html => window.Turbo.renderStreamMessage(html), victimHtml)
    await observer.locator(victimSelector).waitFor()
  }
  const attackerHtml = await create(member, "Maya's separate message")
  const attackerId = attackerHtml.match(/data-message-id="(\d+)"/)[1]
  assert.notEqual(attackerId, victimId)
  for (const observer of [page, memberPage]) {
    await observer.evaluate(html => window.Turbo.renderStreamMessage(html), attackerHtml)
    await observer.locator(`#message_${attackerId}`).waitFor()
    assert.equal(await observer.locator(victimSelector).count(), 1)
    assert.match(await observer.locator(victimSelector).innerText(), /Ada Lovelace/)
    assert.ok((await observer.locator(victimSelector).innerText()).includes(victimText))
  }

  const editedText = "Maya edited only her own message"
  const edited = await member.request.patch(`${roomPath}/messages/${attackerId}`, {
    headers: sameOrigin,
    form: { "message[body]": editedText, "message[client_message_id]": victimId },
    maxRedirects: 0,
  })
  assert.ok([302, 303].includes(edited.status()))
  for (const observer of [page, memberPage]) {
    await observer.locator(`#message_${attackerId}`).filter({ hasText: editedText }).waitFor()
    assert.ok((await observer.locator(victimSelector).innerText()).includes(victimText))
  }
  const removed = await member.request.delete(`${roomPath}/messages/${attackerId}`, {
    headers: { ...sameOrigin, Accept: "text/vnd.turbo-stream.html" },
  })
  assert.equal(removed.status(), 200)
  assert.ok((await removed.text()).includes(`target="message_${attackerId}"`))
  for (const observer of [page, memberPage]) {
    await observer.locator(`#message_${attackerId}`).waitFor({ state: "detached" })
    assert.ok((await observer.locator(victimSelector).innerText()).includes(victimText))
    await observer.reload()
    await observer.locator(victimSelector).waitFor()
    assert.equal(await observer.locator(`#message_${attackerId}`).count(), 0)
    assert.ok((await observer.locator(victimSelector).innerText()).includes(victimText))
  }
})

test("rapid messages keep their first author and day divider before a reload", { timeout: 60_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 1280, height: 900 } })
  const newRoom = await context.request.post("/rooms/opens", {
    headers: sameOrigin,
    form: { "room[name]": "Rapid messages" },
    maxRedirects: 0,
  })
  assert.ok([302, 303].includes(newRoom.status()))
  const emptyRoom = new URL(newRoom.headers().location, page.url()).pathname
  for (const room of [roomPath, emptyRoom]) {
    await page.goto(room)
    await page.locator("#composer").waitFor()
    const requests = []
    let notifyQueued
    const matcher = `**${room}/messages`
    await page.route(matcher, async (route) => {
      if (route.request().method() !== "POST") return route.continue()
      requests.push({ body: route.request().postData(), contentType: route.request().headers()["content-type"] })
      // Acknowledge the transport before sending again; Turbo cancels an older in-flight form.
      // Replay these exact requests below, once all optimistic messages are on screen.
      await route.fulfill({ status: 200, contentType: "text/vnd.turbo-stream.html", body: "" })
      notifyQueued?.()
    })
    try {
      for (let index = 0; index < 3; index++) {
        const queued = new Promise((resolve) => { notifyQueued = resolve })
        await page.locator('#composer lexxy-editor [contenteditable="true"]').fill(`Rapid message ${index + 1}`)
        await page.getByRole("button", { name: "Send Message", exact: true }).click()
        await queued
        await page.locator('.messages > .message').filter({ hasText: `Rapid message ${index + 1}` }).waitFor()
      }
      assert.equal(requests.length, 3, "all three optimistic messages precede their server replies")
      for (let index = 0; index < 3; index++) {
        const posted = await context.request.post(`${room}/messages`, {
          headers: { ...sameOrigin, Accept: "text/vnd.turbo-stream.html", "Content-Type": requests[index].contentType },
          data: requests[index].body,
        })
        assert.equal(posted.status(), 200)
        await page.evaluate((html) => window.Turbo.renderStreamMessage(html), await posted.text())
        await page.waitForFunction((count) => document.querySelectorAll('.messages > .message[data-message-id]').length === count, index + 1)
      }
      const messages = page.locator('.messages > .message[data-message-id]')
      const first = messages.first()
      await page.waitForFunction(() => {
        const first = document.querySelector('.messages > .message[data-message-id]')
        return first?.classList.contains('message--first-of-day') && !first.classList.contains('message--threaded')
      })
      assert.equal(await first.locator('.message__author').isVisible(), true)
      assert.equal(await first.locator('.message__day-separator').isVisible(), true)
      for (let index = 1; index < 3; index++) assert.ok((await messages.nth(index).getAttribute("class")).includes("message--threaded"))
    } finally {
      await page.unroute(matcher)
    }
  }
})

test("settings controls, field replacement, and workspace logo upload stay usable", { timeout: 90_000 }, async (t) => {
  const { page, context } = await setUp(t, { viewport: { width: 1311, height: 1152 } })
  await page.waitForLoadState("networkidle")
  assert.equal(await page.locator('#sidebar a[href="/rooms/opens/new"]').count(), 0)
  await page.locator('nav.mb-rail').getByRole('link', { name: 'Admin', exact: true }).click()
  await page.locator('#channel-settings').getByRole('link', { name: 'New room', exact: true }).click()
  const name = page.locator('#room_name')
  await name.waitFor()
  await name.focus()
  await name.pressSequentially('Research')
  assert.equal(await name.inputValue(), 'Research', 'typing replaces the initial room name')
  await page.locator('h1').click()
  await name.click()
  await name.pressSequentially('Design')
  assert.equal(await name.inputValue(), 'Design', 'entering an existing single-line field selects its value')
  await name.click()
  await name.press('End')
  await name.pressSequentially(' team')
  assert.equal(await name.inputValue(), 'Design team', 'a second click still permits ordinary caret editing')
  const pills = await page.locator('.panel .overflow-ellipsis.fill-shade').evaluateAll((nodes) => nodes.map((node) => ({
    horizontal: parseFloat(getComputedStyle(node).paddingInlineStart),
    vertical: parseFloat(getComputedStyle(node).paddingBlockStart),
  })))
  assert.ok(pills.length >= 2)
  for (const pill of pills) assert.ok(pill.horizontal >= 8 && pill.vertical >= 3, 'membership pills have breathing room')

  await page.locator('nav.mb-rail a[href="/account/edit"]').click()
  await page.getByRole('dialog', { name: 'Unsaved Changes', exact: true }).getByRole('button', { name: 'Discard', exact: true }).click()
  await page.waitForURL('**/account/edit')
  assert.equal(await page.locator('input[type="file"][name="account[logo]"]').count(), 1, 'one logo picker')
  const picker = page.locator('.mb-logo-picker')
  const preview = page.locator('.mb-logo-preview')
  const camera = page.locator('.mb-logo-camera')
  const [pickerBox, previewBox, cameraBox] = await Promise.all([picker.boundingBox(), preview.boundingBox(), camera.boundingBox()])
  assert.equal(pickerBox.width, 96)
  assert.equal(previewBox.width, pickerBox.width)
  assert.ok(cameraBox.x > pickerBox.x && cameraBox.x < pickerBox.x + pickerBox.width, 'camera overlays the image')
  assert.ok(cameraBox.y > pickerBox.y && cameraBox.y < pickerBox.y + pickerBox.height)
  const uploaded = page.waitForResponse((response) => response.request().method() === 'POST' && /^\/account(?:\.|$)/.test(new URL(response.url()).pathname))
  await page.locator('#account_logo').setInputFiles(path.join(repo, 'reference/test/fixtures/files/earth.png'))
  assert.ok([302, 303].includes((await uploaded).status()))
  await page.getByRole('button', { name: 'Remove logo', exact: true }).waitFor()
  const railLogo = page.locator('.mb-rail__brand img')
  await railLogo.waitFor()
  assert.equal(await railLogo.getAttribute('src'), await preview.getAttribute('src'))
  assert.match(await railLogo.getAttribute('src'), /^\/account\/logo\?/)
  assert.equal(await page.locator('.mb-rail__brand .mb-brand-logo').count(), 0, 'custom workspace logos take precedence over Ember artwork')
  assert.equal(await railLogo.evaluate(async image => { await image.decode(); return image.naturalWidth > 0 }), true)
  const stockLogo = await fs.readFile(path.join(repo, 'crates/assets/overrides/logos/app-icon.png'))
  assert.notDeepEqual(await (await context.request.get(await railLogo.getAttribute('src'))).body(), stockLogo)
  await page.reload()
  await page.getByRole('button', { name: 'Remove logo', exact: true }).waitFor()
  assert.equal(await preview.evaluate(async (image) => { await image.decode(); return image.naturalWidth > 0 }), true)

  for (const setting of ['Must be admin to create new rooms', 'Hide translation buttons']) {
    const checkbox = page.getByRole('checkbox', { name: setting, exact: true })
    const checked = await checkbox.isChecked()
    const updated = page.waitForResponse((response) => response.request().method() === 'POST' && /^\/account(?:\.|$)/.test(new URL(response.url()).pathname))
    const redirected = page.waitForResponse((response) => response.request().method() === 'GET' && new URL(response.url()).pathname === '/account/edit' && response.status() === 200)
    await checkbox.locator('..').click()
    assert.ok([302, 303].includes((await updated).status()))
    await redirected
    await page.waitForLoadState('networkidle')
    assert.equal(await page.getByRole('checkbox', { name: setting, exact: true }).isChecked(), !checked)
    assert.equal(await page.locator('.flash').count(), 0, 'immediate switches do not display success toasts')
    await page.reload()
    assert.equal(await page.getByRole('checkbox', { name: setting, exact: true }).isChecked(), !checked, 'setting persists')
  }
  await capture(page, { path: path.join(artifacts, 'workspace-polished-settings.png'), fullPage: true })
  await page.getByRole('button', { name: 'Remove logo', exact: true }).click()
  await page.getByRole('button', { name: 'Remove logo', exact: true }).waitFor({ state: 'detached' })
  await page.reload()
  assert.equal(await page.getByRole('button', { name: 'Remove logo', exact: true }).count(), 0)
  const restoredLogo = page.locator('.mb-rail__brand .mb-brand-logo')
  assert.equal(await restoredLogo.count(), 1)
  assert.match(await restoredLogo.getAttribute('src'), /^\/assets\/ember-icon-[0-9a-f]+\.png$/)
  assert.equal(await restoredLogo.evaluate(async image => { await image.decode(); return image.naturalWidth > 0 }), true)
  assert.equal(await page.locator('.mb-rail__brand svg').count(), 0)
  for (const [url, filename] of [['/account/logo', 'app-icon.png'], ['/account/logo?size=small', 'app-icon-192.png']]) {
    const response = await context.request.get(url)
    assert.equal(response.status(), 200)
    assert.match(response.headers()['content-type'], /^image\/png/)
    assert.deepEqual(await response.body(), await fs.readFile(path.join(repo, 'crates/assets/overrides/logos', filename)))
  }
  await page.setViewportSize({ width: 390, height: 844 })
  await noOverflow(page, 'mobile polished settings')
  await capture(page, { path: path.join(artifacts, 'workspace-polished-settings-mobile.png'), fullPage: true })
})

test("SVG attachments preview as isolated images and keep safe downloads", { timeout: 60_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 1311, height: 900 } })
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180" viewBox="0 0 320 180" onload="parent.svgExecuted=true"><script>parent.svgExecuted=true;fetch('/svg-preview-executed')</script><image href="https://svg-preview.example.test/tracker.png" width="1" height="1"/><rect width="320" height="180" fill="#336699"/><text x="20" y="90" fill="white">Ember SVG</text></svg>`
  const blockedRequests = []
  await context.route('https://svg-preview.example.test/**', (route) => {
    blockedRequests.push(route.request().url())
    return route.abort()
  })
  page.on("request", (request) => {
    if (request.url().includes("/svg-preview-executed")) blockedRequests.push(request.url())
  })
  const uploaded = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === `${roomPath}/messages`)
  await page.locator('#composer input[type="file"]').setInputFiles({ name: "drawing.svg", mimeType: "image/svg+xml", buffer: Buffer.from(svg) })
  await page.getByRole("button", { name: "Send Message", exact: true }).click()
  assert.equal((await uploaded).status(), 200)
  const preview = page.locator('.mb-svg-attachment img[data-svg-preview-target="image"]').first()
  await preview.waitFor({ state: "visible" })
  assert.equal(await preview.evaluate((img) => img.complete && img.naturalWidth === 320), true)
  assert.match(await preview.getAttribute("src"), /^data:image\/svg\+xml;base64,/)
  assert.equal(await page.evaluate(() => window.svgExecuted), undefined)
  assert.deepEqual(blockedRequests, [], "SVG cannot execute scripts or fetch external resources")
  assert.equal(await page.locator(".mb-svg-attachment svg, .mb-svg-attachment object, .mb-svg-attachment iframe").count(), 0)
  const downloadURL = await page.locator('.mb-svg-attachment a').getAttribute("href")
  const download = await context.request.get(downloadURL)
  assert.equal(download.status(), 200)
  assert.match(download.headers()["content-type"], /application\/octet-stream/)
  assert.match(download.headers()["content-disposition"], /^attachment/)
  assert.equal(await download.text(), svg)
  await page.goto("/account/edit")
  await page.goBack()
  await preview.waitFor({ state: "visible" })
  await page.reload()
  await preview.waitFor({ state: "visible" })
  await capture(page, { path: path.join(artifacts, "workspace-svg-preview.png"), fullPage: true })
  for (const width of [320, 390]) {
    await page.setViewportSize({ width, height: 844 })
    await noOverflow(page, `${width}px SVG preview`)
  }
  const invalid = await context.request.post(`${roomPath}/messages`, {
    headers: { ...sameOrigin, Accept: "text/vnd.turbo-stream.html" },
    multipart: { "message[attachment]": { name: "invalid.svg", mimeType: "image/svg+xml", buffer: Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"><broken>') } },
  })
  assert.equal(invalid.status(), 200)
  await page.reload()
  const fallback = page.locator('.message[data-message-id]').filter({ hasText: "invalid.svg" })
  await fallback.waitFor()
  assert.equal(await fallback.locator('img[data-svg-preview-target="image"]').isVisible(), false)
  assert.equal(await fallback.getByRole("link", { name: "Download invalid.svg" }).count(), 1)
  const noJS = await browser.newContext({ baseURL: new URL(page.url()).origin, javaScriptEnabled: false, storageState: await context.storageState() })
  t.after(() => noJS.close())
  const plain = await noJS.newPage()
  await plain.goto(`${roomPath}/messages`)
  assert.equal(await plain.getByRole("link", { name: "Download drawing.svg", exact: true }).count(), 1)
  assert.equal(await plain.locator('.mb-svg-attachment img[data-svg-preview-target="image"]').first().isVisible(), false)
})

test("starred room ordering persists, remains isolated, and rejects unstarred rooms", { timeout: 90_000 }, async (t) => {
  const { context: admin, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1311, height: 900 } })
  for (const name of ["Zulu", "Alpha"]) {
    const response = await admin.request.post("/rooms/opens", { headers: sameOrigin, form: { "room[name]": name } })
    assert.equal(response.status(), 200)
  }
  const { page, context } = await newPage(t, server, { viewport: { width: 1311, height: 900 } })
  assert.equal((await context.request.post(joinURL, { headers: sameOrigin,
    multipart: { "user[name]": "Personal Order", "user[email_address]": "personal@example.test", "user[password]": password },
  })).status(), 200)
  await page.goto(roomPath)
  const names = () => page.locator('#shared_rooms [data-channel-order-target="room"]').evaluateAll((rows) => rows.map((row) => row.dataset.sortedListName))
  await page.waitForFunction(() => document.querySelectorAll('#shared_rooms [data-channel-order-target="room"]').length === 3)
  const alphabetic = await names()
  assert.deepEqual(alphabetic, ["All Talk", "Alpha", "Zulu"])
  const ids = await page.locator('#shared_rooms a').evaluateAll(rows => rows.map(row => Number(row.dataset.roomId)))
  assert.equal((await context.request.put('/users/me/sidebar/order', { headers: sameOrigin, data: { room_ids: ids } })).status(), 403, 'unstarred rooms cannot be reordered')
  for (const id of ids) assert.equal((await context.request.put(`/rooms/${id}/favorite`, { headers: { ...sameOrigin, Accept: 'application/json' }, form: { favorite: 'true' } })).status(), 200)
  await page.reload()
  const zulu = page.locator('#shared_rooms [data-sorted-list-name="Zulu"]')
  await page.waitForFunction(() => window.Stimulus?.getControllerForElementAndIdentifier(document.querySelector('#sidebar_channels'), 'channel-order'))
  const first = await page.locator('#shared_rooms a').first().boundingBox()
  const from = await zulu.boundingBox()
  await page.mouse.move(from.x + 50, from.y + from.height / 2)
  await page.mouse.down()
  await zulu.locator('xpath=self::*[contains(@class,"mb-channel-dragging")]').waitFor()
  await page.evaluate(async () => {
    const frame = document.querySelector('#user_sidebar')
    await frame.loaded
    frame.reload()
    await frame.loaded
  })
  assert.equal(await zulu.evaluate(row => row.classList.contains('mb-channel-dragging')), true, 'a background sidebar refresh preserves an active drag')
  const saved = page.waitForResponse((response) => response.request().method() === "PUT" && new URL(response.url()).pathname === "/users/me/sidebar/order")
  await page.mouse.move(first.x + 50, first.y + 2, { steps: 8 })
  await page.mouse.up()
  assert.equal((await saved).status(), 204)
  assert.deepEqual(await names(), ["Zulu", "All Talk", "Alpha"])
  assert.equal(new URL(page.url()).pathname, roomPath, "drag release must not navigate")
  await page.reload()
  await zulu.waitFor()
  assert.deepEqual(await names(), ["Zulu", "All Talk", "Alpha"])
  const { page: memberPage, context: member } = await newPage(t, server)
  assert.equal((await member.request.post(joinURL, {
    headers: sameOrigin,
    multipart: { "user[name]": "Grace Hopper", "user[email_address]": "grace@example.test", "user[password]": password },
  })).status(), 200)
  await memberPage.goto(roomPath)
  await memberPage.locator('#shared_rooms [data-sorted-list-name="Zulu"]').waitFor()
  assert.deepEqual(await memberPage.locator('#shared_rooms a').evaluateAll((rows) => rows.map((row) => row.dataset.sortedListName)), alphabetic, "one user's ordering cannot reorder another user's sidebar")
  const keyboardSaved = page.waitForResponse((response) => response.request().method() === "PUT" && new URL(response.url()).pathname === "/users/me/sidebar/order")
  await zulu.focus()
  await page.keyboard.press("Alt+ArrowDown")
  assert.equal((await keyboardSaved).status(), 204)
  assert.deepEqual(await names(), ["All Talk", "Zulu", "Alpha"])
  const bad = await context.request.put("/users/me/sidebar/order", { headers: sameOrigin, data: { room_ids: [999999] } })
  assert.equal(bad.status(), 403)
  assert.equal((await context.request.put("/users/me/sidebar/order", { headers: { "Sec-Fetch-Site": "cross-site" }, data: { room_ids: [] } })).status(), 422)
  await context.route('**/users/me/sidebar/order', (route) => route.fulfill({ status: 503 }))
  await page.locator('#shared_rooms [data-sorted-list-name="Alpha"]').focus()
  await page.keyboard.press("Alt+ArrowUp")
  await page.getByText("Couldn’t save room order. Please try again.", { exact: true }).waitFor()
  assert.deepEqual(await names(), ["All Talk", "Zulu", "Alpha"], "failed saves restore the last saved order")
  await context.unroute('**/users/me/sidebar/order')

  // Activity filters read favorites out of view, but ordering must submit all favorites.
  const byName = await page.locator('#shared_rooms a').evaluateAll(rows => Object.fromEntries(rows.map(row => [row.dataset.sortedListName, Number(row.dataset.roomId)])))
  for (const name of ['Alpha', 'Zulu']) {
    assert.equal((await admin.request.post(`/rooms/${byName[name]}/messages`, { headers: { ...sameOrigin, Accept: 'text/vnd.turbo-stream.html' }, form: { 'message[body]': `Unread in ${name}` } })).status(), 200)
  }
  await page.locator('#shared_rooms a[data-sorted-list-name="Alpha"].unread').waitFor()
  await page.locator('#shared_rooms a[data-sorted-list-name="Zulu"].unread').waitFor()
  await page.locator('[data-ember-activity]').click()
  assert.equal(await page.locator('#shared_rooms a[data-sorted-list-name="All Talk"]').isVisible(), false)
  const filteredSave = page.waitForResponse(response => response.request().method() === 'PUT' && new URL(response.url()).pathname === '/users/me/sidebar/order')
  await page.locator('#shared_rooms a[data-sorted-list-name="Alpha"]').press('Alt+ArrowUp')
  const filteredResponse = await filteredSave
  assert.equal(filteredResponse.status(), 204)
  assert.equal(filteredResponse.request().postDataJSON().room_ids.length, 3, 'hidden favorites remain in the saved order')
  await page.locator('[data-ember-activity]').click()

  // A failed reorder must not undo an unstar completed while the request was pending.
  let releaseFailure
  let requestArrived
  const pending = new Promise(resolve => { requestArrived = resolve })
  const failure = new Promise(resolve => { releaseFailure = resolve })
  await page.route('**/users/me/sidebar/order', async route => { requestArrived(); await failure; await route.fulfill({ status: 503 }) })
  await zulu.press('Alt+ArrowUp')
  await pending
  await page.getByRole('button', { name: 'Favorite room', exact: true }).click()
  await page.waitForFunction(() => document.querySelector('.mb-favorite-button').getAttribute('aria-pressed') === 'false')
  releaseFailure()
  await page.getByText("Couldn’t save room order. Please try again.", { exact: true }).waitFor()
  assert.equal(await page.locator('#shared_rooms a[data-sorted-list-name="All Talk"] .mb-room-favorite-marker').isVisible(), false)
  assert.equal(await page.locator('#shared_rooms .mb-room-favorite-marker:visible').count(), 2)
  await page.unroute('**/users/me/sidebar/order')
  const recovered = page.waitForResponse(response => response.request().method() === 'PUT' && new URL(response.url()).pathname === '/users/me/sidebar/order')
  await zulu.press('Alt+ArrowUp')
  assert.equal((await recovered).status(), 204, 'the next reorder uses the newer favorite set')
  for (const id of ids) assert.equal((await context.request.put(`/rooms/${id}/favorite`, { headers: { ...sameOrigin, Accept: 'application/json' }, form: { favorite: 'false' } })).status(), 200)
  await page.reload()
  await zulu.waitFor()
  assert.deepEqual(await names(), alphabetic)
  assert.equal(await page.locator('#shared_rooms .mb-room-drag-handle:visible').count(), 0)
})

test('channel favorites persist per user, update the sidebar, and recover from failed saves', { timeout: 90_000 }, async (t) => {
  const { page, context, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1311, height: 900 } })
  const created = await context.request.post('/rooms/opens', { headers: sameOrigin, form: { 'room[name]': 'Zulu' } })
  assert.equal(created.status(), 200)
  const zuluPath = new URL(created.url()).pathname
  await page.goto(zuluPath)
  const star = page.getByRole('button', { name: 'Favorite room', exact: true })
  const names = () => page.locator('#shared_rooms a').evaluateAll(rows => rows.map(row => row.dataset.sortedListName))
  await page.locator('#shared_rooms a').nth(1).waitFor()
  assert.deepEqual(await names(), ['All Talk', 'Zulu'])
  assert.equal(await star.getAttribute('aria-pressed'), 'false')
  await star.click()
  await page.waitForFunction(() => document.querySelector('.mb-favorite-button')?.getAttribute('aria-pressed') === 'true')
  assert.deepEqual(await names(), ['Zulu', 'All Talk'])
  assert.equal(await page.locator('#shared_rooms a').first().getByRole('img', { name: 'Favorite', exact: true }).isVisible(), true)
  await page.reload()
  await page.locator('#shared_rooms a').nth(1).waitFor()
  assert.equal(await star.getAttribute('aria-pressed'), 'true')
  assert.deepEqual(await names(), ['Zulu', 'All Talk'])
  await page.locator('nav.mb-rail').getByRole('link', { name: 'Search', exact: true }).click()
  await page.getByRole('searchbox').waitFor()
  await page.goBack()
  await star.waitFor()
  assert.equal(await star.getAttribute('aria-pressed'), 'true', 'back navigation preserves the favorite')

  const { page: memberPage, context: member } = await newPage(t, server)
  assert.equal((await member.request.post(joinURL, { headers: sameOrigin,
    multipart: { 'user[name]': 'Grace Hopper', 'user[email_address]': 'favorite-member@example.test', 'user[password]': password },
  })).status(), 200)
  await memberPage.goto(zuluPath)
  await memberPage.locator('#shared_rooms a').nth(1).waitFor()
  assert.equal(await memberPage.getByRole('button', { name: 'Favorite room', exact: true }).getAttribute('aria-pressed'), 'false')
  assert.deepEqual(await memberPage.locator('#shared_rooms a').evaluateAll(rows => rows.map(row => row.dataset.sortedListName)), ['All Talk', 'Zulu'])
  assert.equal((await context.request.put(`${zuluPath}/favorite`, { headers: { 'Sec-Fetch-Site': 'cross-site' }, form: { favorite: 'false' } })).status(), 422)
  assert.equal((await context.request.put('/rooms/999999/favorite', { headers: sameOrigin, form: { favorite: 'true' } })).status(), 403)
  assert.equal((await context.request.put(`${zuluPath}/favorite`, { headers: sameOrigin, form: { favorite: 'invalid' } })).status(), 400)

  await page.route(`**${zuluPath}/favorite`, route => route.fulfill({ status: 503 }))
  await star.click()
  await page.getByText('Couldn’t save your favorite. Please try again.', { exact: true }).waitFor()
  assert.equal(await star.getAttribute('aria-pressed'), 'true')
  assert.deepEqual(await names(), ['Zulu', 'All Talk'])
  await page.unroute(`**${zuluPath}/favorite`)
  await page.setViewportSize({ width: 390, height: 844 })
  await noOverflow(page, 'mobile favorite button')
  await capture(page, { path: path.join(artifacts, 'workspace-favorite-mobile.png') })
  await star.click()
  await page.waitForFunction(() => document.querySelector('.mb-favorite-button')?.getAttribute('aria-pressed') === 'false')
  assert.deepEqual(await names(), ['All Talk', 'Zulu'])
  await page.reload()
  assert.equal(await star.getAttribute('aria-pressed'), 'false')

  const noJS = await browser.newContext({ baseURL: server.origin, javaScriptEnabled: false, storageState: await context.storageState() })
  t.after(() => noJS.close())
  const plain = await noJS.newPage()
  await plain.goto(roomPath)
  await plain.getByRole('button', { name: 'Favorite room', exact: true }).click()
  await plain.waitForLoadState('load')
  assert.equal(await plain.getByRole('button', { name: 'Favorite room', exact: true }).getAttribute('aria-pressed'), 'true', 'ordinary form works without JavaScript')
})

test('mobile starred room ordering supports real touch holds and cancellation', { timeout: 60_000 }, async (t) => {
  const { context, roomPath, server } = await setUp(t)
  for (const name of ['Zulu', 'Alpha']) {
    const response = await context.request.post('/rooms/opens', { headers: sameOrigin, form: { 'room[name]': name } })
    assert.equal(response.status(), 200)
  }
  const sidebar = await context.request.get('/users/me/sidebar')
  const roomIds = [...(await sidebar.text()).matchAll(/data-room-id="(\d+)"/g)].map(match => Number(match[1]))
  for (const id of roomIds) await context.request.put(`/rooms/${id}/favorite`, { headers: { ...sameOrigin, Accept: 'application/json' }, form: { favorite: 'true' } })
  const { page: phone, context: phoneContext } = await newPage(t, server, {
    viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, storageState: await context.storageState(),
  })
  await phone.goto(roomPath)
  await phone.getByRole('button', { name: 'Open conversations', exact: true }).click()
  const phoneZulu = phone.locator('#shared_rooms [data-sorted-list-name="Zulu"]')
  await phoneZulu.waitFor()
  await phone.evaluate(async () => {
    // CDP injects raw coordinates, so wait for both the initial frame and its connection
    // refresh before measuring them. Locator visibility alone does not wait for that render.
    const frame = document.querySelector('#user_sidebar')
    await frame.loaded
    await new Promise(requestAnimationFrame)
    await frame.loaded
    await Promise.all(document.getAnimations().filter(animation => Number.isFinite(animation.effect?.getComputedTiming().endTime)).map(animation => animation.finished.catch(() => {})))
  })
  await phoneZulu.click({ trial: true })
  await phone.waitForFunction(() => window.Stimulus?.getControllerForElementAndIdentifier(document.querySelector('#sidebar_channels'), 'channel-order'))
  const cdp = await phoneContext.newCDPSession(phone)
  const touchPoint = box => ({ x: box.x + 50, y: box.y + box.height / 2, radiusX: 4, radiusY: 4, id: 0 })
  const phoneStart = touchPoint(await phoneZulu.boundingBox())
  const phoneFirst = touchPoint(await phone.locator('#shared_rooms a').first().boundingBox())
  phoneFirst.y -= 8
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [phoneStart] })
  await phone.locator('.mb-channel-dragging').waitFor()
  await phone.evaluate(async () => {
    const frame = document.querySelector('#user_sidebar')
    await frame.loaded
    frame.reload()
    await frame.loaded
  })
  assert.equal(await phoneZulu.evaluate(row => row.classList.contains('mb-channel-dragging')), true, 'a sidebar refresh preserves an active native touch hold')
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [phoneFirst] })
  const touchSaved = phone.waitForResponse(response => response.request().method() === 'PUT' && new URL(response.url()).pathname === '/users/me/sidebar/order')
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  assert.equal((await touchSaved).status(), 204)
  const phoneNames = () => phone.locator('#shared_rooms a').evaluateAll(rows => rows.map(row => row.dataset.sortedListName))
  assert.deepEqual(await phoneNames(), ['Zulu', 'All Talk', 'Alpha'])
  assert.equal(new URL(phone.url()).pathname, roomPath, 'touch release does not follow the channel link')

  let cancelWrites = 0
  phone.on('request', request => {
    if (request.method() === 'PUT' && new URL(request.url()).pathname === '/users/me/sidebar/order') cancelWrites++
  })
  const alphaStart = touchPoint(await phone.locator('#shared_rooms [data-sorted-list-name="Alpha"]').boundingBox())
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [alphaStart] })
  await phone.locator('.mb-channel-dragging').waitFor()
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [phoneFirst] })
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchCancel', touchPoints: [] })
  await phone.getByText('Room move canceled.', { exact: true }).waitFor({ state: 'attached' })
  assert.deepEqual(await phoneNames(), ['Zulu', 'All Talk', 'Alpha'], 'touch cancellation restores the saved order')
  assert.equal(cancelWrites, 0, 'canceled touch reorder never writes a preference')
  await phone.reload()
  await phone.getByRole('button', { name: 'Open conversations', exact: true }).click()
  await phoneZulu.waitFor()
  assert.deepEqual(await phoneNames(), ['Zulu', 'All Talk', 'Alpha'])
  await noOverflow(phone, 'mobile channel ordering')
  await capture(phone, { path: path.join(artifacts, 'workspace-channel-ordering-mobile.png'), fullPage: true })
})

test('starred room ordering scrolls long lists and updates the drop position at a stationary edge', { timeout: 60_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 1311, height: 500 } })
  for (let index = 1; index <= 24; index++) {
    const response = await context.request.post('/rooms/opens', {
      headers: sameOrigin, form: { 'room[name]': `Channel ${String(index).padStart(2, '0')}` },
    })
    assert.equal(response.status(), 200)
  }
  const sidebar = await context.request.get('/users/me/sidebar')
  const roomIds = [...(await sidebar.text()).matchAll(/data-room-id="(\d+)"/g)].map(match => Number(match[1]))
  for (const id of roomIds) await context.request.put(`/rooms/${id}/favorite`, { headers: { ...sameOrigin, Accept: 'application/json' }, form: { favorite: 'true' } })
  await page.reload()
  await page.waitForFunction(() => document.querySelectorAll('#shared_rooms a').length === 25 &&
    window.Stimulus?.getControllerForElementAndIdentifier(document.querySelector('#sidebar_channels'), 'channel-order'))
  await page.evaluate(async () => {
    const frame = document.querySelector('#user_sidebar')
    await frame.loaded
    await new Promise(requestAnimationFrame)
    await frame.loaded
  })
  const first = page.locator('#shared_rooms [data-sorted-list-name="All Talk"]')
  await first.click({ trial: true })
  const initial = await first.boundingBox()
  const scroller = page.locator('.mb-conversations')
  const bounds = await scroller.boundingBox()
  assert.equal(await scroller.evaluate(element => element.scrollHeight > element.clientHeight), true)
  await page.mouse.move(initial.x + 50, initial.y + initial.height / 2)
  await page.mouse.down()
  await page.locator('.mb-channel-dragging').waitFor()
  await page.mouse.move(initial.x + 50, bounds.y + bounds.height - 20, { steps: 4 })
  const firstDropIndex = await first.evaluate(row => [...row.parentElement.children].indexOf(row))
  await page.waitForFunction(index => {
    const row = document.querySelector('.mb-channel-dragging')
    return document.querySelector('.mb-conversations').scrollTop >= 240 &&
      [...row.parentElement.children].indexOf(row) >= index + 4
  }, firstDropIndex)
  const beforeDrop = await first.evaluate(row => [...row.parentElement.children].indexOf(row))
  assert.ok(beforeDrop >= firstDropIndex + 4, 'stationary pointer advances through channels as the list scrolls')
  const saved = page.waitForResponse(response => response.request().method() === 'PUT' && new URL(response.url()).pathname === '/users/me/sidebar/order')
  await page.mouse.up()
  assert.equal((await saved).status(), 204)
  assert.equal(new URL(page.url()).pathname, roomPath)
  const order = await page.locator('#shared_rooms a').evaluateAll(rows => rows.map(row => row.dataset.sortedListName))
  await page.reload()
  await page.waitForFunction(() => document.querySelectorAll('#shared_rooms a').length === 25)
  assert.deepEqual(await page.locator('#shared_rooms a').evaluateAll(rows => rows.map(row => row.dataset.sortedListName)), order)
  await noOverflow(page, 'long channel ordering list')
})

test("notification bell explains missing configuration and shows the real release version", { timeout: 30_000 }, async (t) => {
  const { page, context } = await setUp(t)
  await page.getByRole("button", { name: "Enable notifications", exact: true }).click()
  const dialog = page.getByRole("dialog", { name: "Notification setup" })
  await dialog.waitFor()
  assert.match(await dialog.innerText(), /Notifications aren’t (configured|available here)/)
  await dialog.getByRole("button", { name: "Close", exact: true }).click()
  await dialog.waitFor({ state: "hidden" })
  await page.goto("/account/edit")
  assert.match(await page.locator("#footer").innerText(), /Ember™ version 1\.0/)
  assert.equal((await context.request.get("/account/edit")).headers()["x-version"], "1.0")
})

test("follow-up timestamps sit after message content without shifting it", { timeout: 90_000 }, async (t) => {
  const { page, roomPath } = await setUp(t, { viewport: { width: 1311, height: 1152 } })
  await page.waitForLoadState('networkidle')
  const first = await submitMessage(page, roomPath, 'A new message group.')
  const second = await submitMessage(page, roomPath, 'A short follow-up.')
  await page.waitForFunction(() => [...document.querySelectorAll('.message--threaded')].some(node => node.textContent.includes('A short follow-up.')))
  await second.scrollIntoViewIfNeeded()
  await page.locator('#nav').hover()
  const textBounds = async () => second.locator('[data-reply-target="body"] .lexxy-content').evaluate(node => {
    const range = document.createRange()
    range.selectNodeContents(node)
    const box = range.getBoundingClientRect()
    return { x: box.x, y: box.y, right: box.right, bottom: box.bottom }
  })
  const before = await textBounds()
  assert.equal(await second.locator('.message__permalink').evaluate(node => getComputedStyle(node).opacity), '0')
  await second.hover()
  const after = await textBounds()
  assert.deepEqual(after, before, 'hover does not move message text')
  const time = await second.locator('.message__permalink').boundingBox()
  assert.ok(time.x >= after.right + 7 && time.x <= after.right + 14, `timestamp follows the text: ${JSON.stringify({ time, after })}`)
  assert.ok(time.y >= after.y - 3 && time.y <= after.bottom, 'timestamp aligns with its message line')
  assert.equal(await second.locator('.message__permalink').evaluate(node => getComputedStyle(node).opacity), '1')
  assert.equal(await first.locator('.message__permalink').evaluate(node => getComputedStyle(node).opacity), '1', 'group header timestamp remains visible')
  await capture(page, { path: path.join(artifacts, 'workspace-inline-timestamp.png'), fullPage: true })
  await page.locator('#nav').hover()
  await second.locator('.message__permalink').focus()
  assert.equal(await second.locator('.message__permalink').evaluate(node => getComputedStyle(node).opacity), '1', 'keyboard focus reveals the permalink')

  await second.hover()
  await second.locator('summary').filter({ hasText: 'Message options' }).click()
  const boosted = page.waitForResponse(response => response.request().method() === 'POST' && /\/boosts$/.test(new URL(response.url()).pathname))
  await second.getByRole('button', { name: 'Thumbs up', exact: true }).click()
  assert.ok([302, 303].includes((await boosted).status()))
  await second.locator('.boost').first().waitFor()
  const reactedText = await textBounds()
  const reactedTime = await second.locator('.message__permalink').boundingBox()
  assert.ok(reactedTime.x >= reactedText.right + 7 && reactedTime.x <= reactedText.right + 14, 'reactions do not push the timestamp away from its text')

  await submitMessage(page, roomPath, 'Long text wraps safely beside its time. '.repeat(12) + 'https://example.test/' + 'a'.repeat(160))
  const uploaded = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === `${roomPath}/messages`)
  await page.locator('#composer input[type="file"]').setInputFiles(path.join(repo, 'reference/test/fixtures/files/earth.png'))
  await page.getByRole('button', { name: 'Send Message', exact: true }).click()
  assert.equal((await uploaded).status(), 200)
  const imageMessage = page.locator('.message[data-message-id]').filter({ has: page.locator('img.message__attachment') }).last()
  await imageMessage.waitFor()
  for (const width of [1311, 390, 320]) {
    await page.setViewportSize({ width, height: 844 })
    await noOverflow(page, `${width}px inline timestamps`)
    // Attachment updates can replace the message while Playwright waits for a stable element.
    await imageMessage.evaluate((node) => node.scrollIntoView({ block: "center", behavior: "instant" }))
    const image = await imageMessage.locator('img.message__attachment').boundingBox()
    const stamp = await imageMessage.locator('.message__permalink').boundingBox()
    assert.ok(stamp.x >= image.x + image.width && stamp.x + stamp.width <= width - 8, `${width}px attachment timestamp stays beside the image and inside the viewport: ${JSON.stringify({ image, stamp })}`)
    const overflowingMessages = await page.locator('.message--threaded [data-reply-target="body"]').evaluateAll(nodes => nodes.filter(node => node.scrollWidth > node.clientWidth + 1).map(node => node.id))
    assert.deepEqual(overflowingMessages, [], `${width}px message contents fit their columns`)
  }
  await capture(page, { path: path.join(artifacts, 'workspace-inline-timestamp-mobile.png'), fullPage: true })
})

test('channel icons can be staged, saved, reset, and authorized without changing other channel data', { timeout: 120_000 }, async (t) => {
  const { page, context, server, roomPath: originalRoomPath, joinURL } = await setUp(t, { viewport: { width: 1311, height: 1152 } })
  await page.waitForLoadState('networkidle')
  const { page: peer } = await newPage(t, server, { storageState: await context.storageState() })
  await peer.goto(originalRoomPath)
  await peer.locator('#shared_rooms a').first().waitFor()
  await peer.waitForLoadState('networkidle')

  let catalogRequests = 0
  page.on('request', request => {
    if (/\/assets\/lucide\/catalog[^/]*\.json$/.test(new URL(request.url()).pathname)) catalogRequests++
  })
  await page.goto('/rooms/opens/new')
  await page.locator('#room_name').fill('Cafe crew')
  assert.equal(catalogRequests, 0, 'channel forms do not fetch the icon catalogue until the picker opens')
  const launcher = page.getByRole('button', { name: 'Choose room icon', exact: true })
  const dialog = page.getByRole('dialog', { name: 'Choose a room icon', exact: true })
  const select = page.locator('#room_icon')
  const choose = async (label) => {
    await dialog.getByRole('searchbox', { name: 'Search icons', exact: true }).fill(label)
    await dialog.getByRole('button', { name: `Use ${label} icon`, exact: true }).click()
  }
  await launcher.click()
  await dialog.waitFor()
  await dialog.getByRole('button', { name: 'Use Coffee icon', exact: true }).waitFor()
  await capture(page, { path: path.join(artifacts, 'workspace-channel-icon-picker-desktop.png'), fullPage: true })
  await choose('Coffee')
  assert.equal(await select.inputValue(), '', 'choosing a candidate does not commit it to the form')
  await dialog.getByRole('button', { name: 'Use icon', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  assert.equal(await select.inputValue(), 'coffee')

  await launcher.click()
  await choose('Rocket')
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  assert.equal(await select.inputValue(), 'coffee', 'cancel preserves the previously selected icon')
  await launcher.click()
  await choose('Star')
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  assert.equal(await select.inputValue(), 'coffee', 'Escape discards the pending selection')
  assert.equal(await launcher.evaluate(node => node === document.activeElement), true, 'Escape restores focus to the launcher')
  assert.equal(catalogRequests, 1, 'reopening the picker reuses the immutable catalogue')
  await page.locator('label[for="room_type"]').click()
  await page.waitForURL(url => url.pathname === '/rooms/closeds/new')
  assert.equal(await select.inputValue(), 'coffee', 'switching to restricted access preserves the unsaved icon')
  assert.equal(await page.locator('#room_name').inputValue(), 'Cafe crew')
  await page.locator('label[for="room_type"]').click()
  await page.waitForURL(url => url.pathname === '/rooms/opens/new')
  assert.equal(await select.inputValue(), 'coffee', 'switching back to open access preserves the unsaved icon')

  const created = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === '/rooms/opens')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  assert.ok([302, 303].includes((await created).status()))
  await page.waitForURL(url => /^\/rooms\/\d+$/.test(url.pathname))
  const roomPath = new URL(page.url()).pathname
  const roomId = roomPath.split('/').at(-1)
  const editPath = `/rooms/opens/${roomId}/edit`
  const assertIcon = async (target, icon) => {
    await target.waitForFunction(({ roomPath, icon }) =>
      document.querySelector('.mb-room-symbol')?.getAttribute('data-channel-icon') === icon &&
      document.querySelector(`#shared_rooms a[href="${roomPath}"] .mb-room-kind`)?.getAttribute('data-channel-icon') === icon,
    { roomPath, icon })
  }
  await assertIcon(page, 'coffee')
  await peer.locator(`#shared_rooms a[href="${roomPath}"] .mb-room-kind[data-channel-icon="coffee"]`).waitFor()
  await page.reload()
  await assertIcon(page, 'coffee')
  await peer.goto(roomPath)
  await assertIcon(peer, 'coffee')
  await peer.waitForLoadState('networkidle')

  await page.goto(editPath)
  assert.equal(await select.inputValue(), 'coffee')
  await launcher.click()
  await choose('Rocket')
  await dialog.getByRole('button', { name: 'Use icon', exact: true }).click()
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await page.waitForURL(url => url.pathname === roomPath)
  await assertIcon(page, 'rocket')
  await assertIcon(peer, 'rocket')
  await page.reload()
  await assertIcon(page, 'rocket')
  await capture(page, { path: path.join(artifacts, 'workspace-channel-icon.png'), fullPage: true })

  const { page: memberPage, context: member } = await newPage(t, server)
  const joined = await member.request.post(joinURL, {
    headers: sameOrigin,
    multipart: { 'user[name]': 'Grace Hopper', 'user[email_address]': 'grace-icons@example.test', 'user[password]': password },
  })
  assert.equal(joined.status(), 200)
  await memberPage.goto(editPath)
  assert.equal(await memberPage.getByRole('button', { name: 'Choose room icon', exact: true }).count(), 0, 'members cannot open an unauthorized editor')
  for (const type of ['opens', 'closeds']) {
    const denied = await member.request.patch(`/rooms/${type}/${roomId}`, {
      headers: sameOrigin, form: { 'room[name]': 'Unauthorized rename', 'room[icon]': 'star' }, maxRedirects: 0,
    })
    assert.equal(denied.status(), 403, 'only channel creators and administrators can change channel icons')
  }
  for (const form of [
    { 'room[name]': 'Invalid rename', 'room[icon]': 'not-a-lucide-icon' },
    { 'room[name]': 'Invalid rename', 'room[icon][]': 'coffee' },
  ]) {
    const invalid = await context.request.patch(`/rooms/closeds/${roomId}`, { headers: sameOrigin, form, maxRedirects: 0 })
    assert.equal(invalid.status(), 422, 'unknown icons and wrong-shaped values reject the entire update')
  }
  await page.reload()
  await assertIcon(page, 'rocket')
  assert.equal(await page.locator('#nav .mb-room-heading h1').innerText(), 'Cafe crew', 'rejected updates do not change the channel name')
  assert.equal(await page.locator('#nav a[title="Conversation settings"]').getAttribute('href'), editPath, 'rejected updates do not change channel type')
  assert.equal((await member.request.get(roomPath)).status(), 200, 'rejected closed updates preserve access')

  const ownRoom = await member.request.post('/rooms/opens', {
    headers: sameOrigin, form: { 'room[name]': 'Grace’s channel', 'room[icon]': 'star' },
  })
  assert.equal(ownRoom.status(), 200)
  const ownRoomPath = new URL(ownRoom.url()).pathname
  const ownRoomId = ownRoomPath.split('/').at(-1)
  const creatorSaved = await member.request.patch(`/rooms/opens/${ownRoomId}`, {
    headers: sameOrigin, form: { 'room[icon]': 'coffee' }, maxRedirects: 0,
  })
  assert.ok([302, 303].includes(creatorSaved.status()), 'non-admin channel creators retain editing permission')
  await memberPage.goto(ownRoomPath)
  await memberPage.locator('.mb-room-symbol[data-channel-icon="coffee"]').waitFor()

  await page.goto(editPath)
  await launcher.click()
  await dialog.getByRole('button', { name: 'Use default icon', exact: true }).click()
  assert.equal(await select.inputValue(), 'rocket', 'reset remains pending until applied')
  await dialog.getByRole('button', { name: 'Use icon', exact: true }).click()
  assert.equal(await select.inputValue(), '')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await page.waitForURL(url => url.pathname === roomPath)
  await assertIcon(page, '')
  await page.reload()
  await assertIcon(page, '')
})

test('channel icon picker fits small dark screens and closed channels work without JavaScript', { timeout: 90_000 }, async (t) => {
  const { page, context, server } = await setUp(t, { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, colorScheme: 'dark' })
  await page.goto('/rooms/opens/new')
  await page.locator('#room_name').fill('Mobile ideas')
  const launcher = page.getByRole('button', { name: 'Choose room icon', exact: true })
  const dialog = page.getByRole('dialog', { name: 'Choose a room icon', exact: true })
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 })
    await launcher.click()
    await dialog.waitFor()
    const bounds = await dialog.boundingBox()
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width, `${width}px picker fits the viewport`)
    assert.ok(bounds.y >= 0 && bounds.y + bounds.height <= 844, `${width}px picker fits the screen height`)
    await noOverflow(page, `${width}px icon picker`)
    const search = dialog.getByRole('searchbox', { name: 'Search icons', exact: true })
    await search.fill('not-a-real-lucide-icon')
    assert.equal(await dialog.getByRole('button', { name: /^Use .+ icon$/ }).filter({ visible: true }).count(), 1, 'empty results retain only the default-icon action')
    await search.fill('Coffee')
    await dialog.getByRole('button', { name: 'Use Coffee icon', exact: true }).waitFor()
    for (const key of ['Tab', 'Tab', 'Tab', 'Tab', 'Tab', 'Shift+Tab']) {
      await page.keyboard.press(key)
      const focus = await dialog.evaluate(node => ({
        inside: node.contains(document.activeElement), tag: document.activeElement.tagName, open: node.open,
      }))
      assert.ok(focus.inside || (focus.tag === 'BODY' && focus.open), 'Tab never focuses a background-page control')
    }
    await capture(page, { path: path.join(artifacts, `workspace-channel-icon-picker-${width}-dark.png`), fullPage: true })
    await page.keyboard.press('Escape')
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(await launcher.evaluate(node => node === document.activeElement), true)
    assert.equal(await page.locator('#room_icon').inputValue(), '', 'dismissal never applies a candidate')
  }
  await launcher.click()
  await dialog.getByRole('searchbox', { name: 'Search icons', exact: true }).fill('Coffee')
  await dialog.getByRole('button', { name: 'Use Coffee icon', exact: true }).click()
  await dialog.getByRole('button', { name: 'Use icon', exact: true }).click()
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await page.waitForURL(url => /^\/rooms\/\d+$/.test(url.pathname))
  await page.locator('.mb-room-symbol[data-channel-icon="coffee"]').waitFor()
  await page.getByRole('button', { name: 'Open conversations', exact: true }).click()
  await page.locator('#shared_rooms a').filter({ hasText: 'Mobile ideas' }).locator('.mb-room-kind[data-channel-icon="coffee"]').waitFor()
  await noOverflow(page, 'mobile channel icon sidebar')

  const { page: plain, context: plainContext } = await newPage(t, server, { javaScriptEnabled: false, storageState: await context.storageState(), viewport: { width: 1000, height: 900 } })
  try {
    await plain.goto('/rooms/closeds/new')
    await plain.locator('#room_name').fill('Private launch')
    const native = plain.locator('#room_icon')
    assert.equal(await native.isVisible(), true, 'the full native icon selector remains usable without JavaScript')
    assert.ok(await native.locator('option').count() > 1000, 'native fallback includes the icon catalogue')
    await native.selectOption('rocket')
    await plain.getByRole('button', { name: 'Save', exact: true }).click()
    await plain.waitForURL(url => /^\/rooms\/\d+$/.test(url.pathname))
    const closedPath = new URL(plain.url()).pathname
    const closedId = closedPath.split('/').at(-1)
    await plain.locator('.mb-room-symbol[data-channel-icon="rocket"]').waitFor()
    await plain.goto(`/rooms/closeds/${closedId}/edit`)
    assert.equal(await native.inputValue(), 'rocket')
    await native.selectOption('star')
    await plain.getByRole('button', { name: 'Save', exact: true }).click()
    await plain.waitForURL(url => url.pathname === closedPath)
    await plain.reload()
    await plain.locator('.mb-room-symbol[data-channel-icon="star"]').waitFor()
    assert.equal(await plain.locator('#nav .mb-room-heading h1').innerText(), 'Private launch')
    assert.equal(await plain.locator('#nav a[title="Conversation settings"]').getAttribute('href'), `/rooms/closeds/${closedId}/edit`)
    await noOverflow(plain, 'no-JavaScript closed channel')
  } finally {
    // The shared screenshot helper awaits animation frames, which disabled JS cannot schedule.
    await plainContext.close()
  }
})

test("compact message actions appear on hover, stay reachable, and dismiss without shifting messages", { timeout: 60_000 }, async (t) => {
  const { page, roomPath } = await setUp(t, { viewport: { width: 1311, height: 900 } })
  const first = await submitMessage(page, roomPath, "Hover over this message.")
  const second = await submitMessage(page, roomPath, "And then this message.")
  await page.locator("#nav").hover()
  const body = first.locator('[data-reply-target="body"]')
  const before = await body.boundingBox()
  const firstBar = first.locator(".mb-message-action-bar")
  const secondBar = second.locator(".mb-message-action-bar")
  await first.hover()
  await firstBar.waitFor()
  assert.deepEqual(await body.boundingBox(), before, "hovering does not move message content")
  const bounds = await firstBar.boundingBox()
  const row = await first.locator(".message__body").boundingBox()
  const pane = await page.locator(".messages").boundingBox()
  const avatar = await first.locator(".message__avatar").boundingBox()
  assert.equal(row.x - pane.x, 64, "desktop message text begins 64px from the pane edge")
  assert.equal(avatar.x - pane.x, 20)
  assert.equal(row.x - avatar.x - avatar.width, 8)
  assert.equal(avatar.y - row.y, 8)
  assert.equal(row.height, 52, "a one-line author/message row has a 52px hover area")
  assert.equal(await first.locator('.message__avatar img').evaluate(node => getComputedStyle(node).borderRadius), '6px')
  assert.equal(bounds.height, 42)
  assert.equal(bounds.width, 266, "the options trigger is included in the compact eight-control bar")
  assert.equal(pane.x + pane.width - bounds.x - bounds.width, 16)
  assert.equal(row.y - bounds.y, 17)
  const optionsBox = await first.locator('.message__options-btn').boundingBox()
  assert.ok(optionsBox.x >= bounds.x && optionsBox.x + optionsBox.width <= bounds.x + bounds.width && optionsBox.y >= bounds.y && optionsBox.y + optionsBox.height <= bounds.y + bounds.height, "the options trigger stays clickable inside the toolbar")
  assert.equal((await first.locator('.message__day-separator time').boundingBox()).height, 28)
  assert.ok(bounds.y < row.y + row.height && bounds.y + bounds.height >= row.y + 12, "the bar overlaps the row rather than floating above it")
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2, { steps: 8 })
  await page.waitForTimeout(250)
  assert.equal(await firstBar.isVisible(), true, "the pointer can cross to the floating bar")
  await capture(page, { path: path.join(artifacts, "workspace-compact-actions-hover.png"), fullPage: true })
  await first.hover()
  await page.waitForTimeout(250)
  assert.equal(await firstBar.isVisible(), true, "returning from the bar to its message keeps it open")
  await second.hover()
  await secondBar.waitFor()
  const secondRow = await second.locator(".message__body").boundingBox()
  const secondBounds = await secondBar.boundingBox()
  assert.ok(secondBounds.y < secondRow.y + secondRow.height && secondBounds.y + secondBounds.height >= secondRow.y + 12, "even a short follow-up overlaps its toolbar")
  const textBounds = await second.locator('[data-reply-target="body"]').boundingBox()
  assert.equal(secondRow.height, 30, "continuation rows retain a 30px rhythm")
  assert.equal(textBounds.height, 22)
  assert.ok(Math.abs((textBounds.y + textBounds.height / 2) - (secondRow.y + secondRow.height / 2)) < 1, "text stays vertically centered within the taller row")
  const barCenter = secondBounds.y + secondBounds.height / 2
  assert.ok(barCenter >= secondRow.y + 3 && barCenter <= secondRow.y + 5, "the toolbar floats at the top edge of the row, above its text")
  const start = { x: secondRow.x + 40, y: secondRow.y + Math.min(12, secondRow.height / 2) }
  const end = { x: secondBounds.x + 20, y: secondBounds.y + secondBounds.height / 2 }
  await page.mouse.move(start.x, start.y)
  for (let step = 1; step <= 8; step++) {
    await page.mouse.move(start.x + (end.x - start.x) * step / 8, start.y + (end.y - start.y) * step / 8)
    await page.waitForTimeout(100)
    assert.equal(await secondBar.isVisible(), true, "slow pointer travel must not rely on the dismissal grace period")
  }
  assert.equal(await firstBar.isVisible(), false)
  assert.equal(await page.locator(".mb-message-actions[open]").count(), 1)
  await capture(page, { path: path.join(artifacts, "workspace-compact-actions-follow-up.png"), fullPage: true })
  await page.locator("#nav").hover()
  await secondBar.waitFor({ state: "hidden" })

  const trigger = first.locator(".message__options-btn")
  await trigger.focus()
  await page.keyboard.press("Enter")
  await firstBar.waitFor()
  await page.locator("#nav").hover()
  assert.equal(await firstBar.isVisible(), true, "keyboard-opened controls do not require hover")
  await page.keyboard.press("Escape")
  await firstBar.waitFor({ state: "hidden" })
  assert.equal(await trigger.evaluate(node => node === document.activeElement), true)

  await page.setViewportSize({ width: 1311, height: 500 })
  await second.scrollIntoViewIfNeeded()
  await second.hover()
  await secondBar.waitFor()
  const lowBar = await secondBar.boundingBox()
  await second.getByRole("button", { name: "More reactions", exact: true }).click()
  const expandedBar = await secondBar.boundingBox()
  const tray = await second.locator(".mb-message-reaction-tray").boundingBox()
  const timeline = await page.locator(".messages").boundingBox()
  assert.ok(Math.abs(expandedBar.y - lowBar.y) < 1, "a tray near the composer keeps its bar anchored")
  assert.ok(tray.y < expandedBar.y && tray.y >= timeline.y, "the tray opens above the bar when space below is limited")
  await capture(page, { path: path.join(artifacts, "workspace-compact-actions-tray-above.png") })
})

test("compact message actions preserve reactions, reply, copying, and keyboard focus", { timeout: 90_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 1311, height: 900 } })
  await context.grantPermissions(["clipboard-read", "clipboard-write"])
  const message = await submitMessage(page, roomPath, "A compact set of message actions.")
  const trigger = message.locator(".message__options-btn")
  const menu = message.locator(".mb-message-actions-menu")
  const bar = message.locator(".mb-message-action-bar")
  const tray = message.locator(".mb-message-reaction-tray")
  const more = menu.getByRole("button", { name: "More reactions", exact: true })
  const open = async () => {
    await page.locator("#nav").hover()
    await message.hover()
    await bar.waitFor()
    await page.waitForFunction(() => document.querySelector('.mb-message-actions[open] > .mb-message-actions-menu')?.style.left)
  }
  await open()
  assert.equal(await tray.isVisible(), false)
  assert.equal(await bar.locator("form").count(), 3)
  assert.equal(await more.locator("svg").count(), 1, "the more-reactions control has a visible icon")
  const bounds = await bar.boundingBox()
  assert.ok(bounds.width < 300 && bounds.height < 52, `compact bar is a single short row: ${JSON.stringify(bounds)}`)
  await capture(page, { path: path.join(artifacts, "workspace-compact-actions-desktop.png"), fullPage: true })

  await menu.getByRole("button", { name: "Copy link", exact: true }).click()
  const link = new URL(await menu.getByRole("button", { name: "Copy link", exact: true }).getAttribute("data-copy-to-clipboard-url-value"), page.url()).href
  await page.waitForFunction(async (link) => await navigator.clipboard.readText() === link, link)
  await more.click()
  assert.equal(await tray.isVisible(), true)
  const expanded = await bar.boundingBox()
  assert.ok(Math.abs(expanded.y - bounds.y) < 1, "expanding reactions does not move the bar off its row")
  await page.keyboard.press("Tab")
  assert.equal(await tray.evaluate((node) => node.contains(document.activeElement)), true)
  await page.keyboard.press("Escape")
  assert.equal(await tray.isVisible(), false)
  assert.equal(await more.evaluate((node) => node === document.activeElement), true)
  await page.keyboard.press("Escape")
  assert.equal(await menu.isVisible(), false)
  assert.equal(await trigger.evaluate((node) => node === document.activeElement), true)

  await page.keyboard.press("Enter")
  await bar.waitFor()
  await more.click()
  const boosted = page.waitForResponse((response) => response.request().method() === "POST" && /\/boosts$/.test(new URL(response.url()).pathname))
  await tray.getByRole("button", { name: "Fire", exact: true }).click()
  assert.ok([302, 303].includes((await boosted).status()))
  await message.locator(".boost").filter({ hasText: "🔥" }).waitFor()
  assert.equal(await menu.isVisible(), false)

  await open()
  await menu.getByRole("button", { name: "Reply", exact: true }).click()
  await page.locator("#composer blockquote").getByText("A compact set of message actions.", { exact: true }).waitFor()
  assert.equal(await menu.isVisible(), false)
  await page.locator('#composer lexxy-editor [contenteditable="true"]').fill("")

  await open()
  await more.click()
  await tray.getByRole("link", { name: "New boost", exact: true }).click()
  await message.locator(".input--boost").fill("Nicely done")
  const customBoost = page.waitForResponse((response) => response.request().method() === "POST" && /\/boosts$/.test(new URL(response.url()).pathname))
  await message.getByRole("button", { name: "Add reaction", exact: true }).click()
  assert.ok([302, 303].includes((await customBoost).status()))
  await message.locator(".boost").filter({ hasText: "Nicely done" }).waitFor()

  await open()
  await more.click()
  await page.locator('#nav a[href="/searches"]').click()
  await page.locator("#q").waitFor()
  await page.goBack()
  await page.locator("#composer").waitFor()
  assert.equal(await page.locator(".mb-message-actions[open]").count(), 0, "Turbo restores closed action bars")
  await open()
  assert.equal(await tray.isVisible(), false)
  await page.locator("#nav").click({ position: { x: 120, y: 20 } })
  assert.equal(await menu.isVisible(), false, "clicking outside closes the bar")
})

test("compact message actions fit mobile edges, attachments, and dark mode", { timeout: 90_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, colorScheme: "dark" })
  await submitMessage(page, roomPath, "A message before the attachment.")
  const response = await context.request.post(`${roomPath}/messages`, {
    headers: { ...sameOrigin, Accept: "text/vnd.turbo-stream.html" },
    multipart: { "message[attachment]": { name: "action-test.txt", mimeType: "text/plain", buffer: Buffer.from("Compact bar download") } },
  })
  assert.equal(response.status(), 200)
  await page.reload()
  const message = page.locator(".message[data-message-id]").filter({ hasText: "action-test.txt" })
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: width === 320 ? 620 : 844 })
    await message.locator(".message__options-btn").click()
    const menu = message.locator(".mb-message-actions-menu")
    await menu.waitFor()
    await menu.getByRole("button", { name: "More reactions", exact: true }).click()
    const bounds = await menu.boundingBox()
    const timeline = await page.locator(".messages").boundingBox()
    const trigger = await message.locator(".message__options-btn").boundingBox()
    assert.ok(bounds.x >= 7 && bounds.x + bounds.width <= width - 7, `${width}px bar and tray fit horizontally: ${JSON.stringify(bounds)}`)
    assert.ok(bounds.y >= timeline.y && bounds.y + bounds.height <= timeline.y + timeline.height, "bar and tray stay above the composer")
    if (trigger.y + trigger.height + bounds.height + 6 > timeline.y + timeline.height - 8) {
      assert.ok(bounds.y < trigger.y, "the bar flips above its trigger when there is no room below")
    }
    assert.equal(await menu.getByRole("button", { name: "Reply", exact: true }).count(), 0)
    assert.equal(await menu.getByRole("link", { name: "Download", exact: true }).isVisible(), true)
    const download = await context.request.get(await menu.getByRole("link", { name: "Download", exact: true }).getAttribute("href"))
    assert.equal(await download.text(), "Compact bar download")
    await noOverflow(page, `${width}px compact message actions`)
    await capture(page, { path: path.join(artifacts, `workspace-compact-actions-${width}-dark.png`), fullPage: true })
    await page.locator("#nav").click({ position: { x: 120, y: 20 } })
    assert.equal(await menu.isVisible(), false)
  }
})

test('profile avatar uses one camera picker, supports upload and removal, and works without JavaScript', { timeout: 90_000 }, async t => {
  const { page, context, server } = await setUp(t, { viewport: { width: 950, height: 914 } })
  await page.goto('/users/me/profile')
  const avatar = page.locator('.mb-profile-avatar')
  assert.equal(await avatar.locator('input[type="file"]').count(), 1)
  const [picture, camera] = await Promise.all([avatar.locator('.mb-logo-preview').boundingBox(), avatar.locator('.mb-logo-camera').boundingBox()])
  assert.equal(picture.width, 96)
  assert.equal(picture.height, 96)
  assert.ok(camera.x > picture.x && camera.x < picture.x + picture.width)
  assert.ok(camera.y > picture.y && camera.y < picture.y + picture.height)
  const uploaded = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === '/users/me/profile')
  await avatar.getByLabel('Upload avatar').setInputFiles(path.join(repo, 'reference/test/fixtures/files/earth.png'))
  assert.ok([302, 303].includes((await uploaded).status()))
  await avatar.getByRole('button', { name: 'Remove avatar', exact: true }).waitFor()
  await page.reload()
  assert.equal(await avatar.locator('.mb-logo-preview').evaluate(async image => { await image.decode(); return image.naturalWidth > 0 }), true)
  await capture(page, { path: path.join(artifacts, 'workspace-profile-avatar.png'), fullPage: true })
  await page.setViewportSize({ width: 390, height: 844 })
  await noOverflow(page, 'mobile profile avatar')
  await capture(page, { path: path.join(artifacts, 'workspace-profile-avatar-mobile.png'), fullPage: true })
  await avatar.getByRole('button', { name: 'Remove avatar', exact: true }).click()
  await avatar.getByRole('button', { name: 'Remove avatar', exact: true }).waitFor({ state: 'detached' })
  const plainContext = await browser.newContext({ baseURL: server.origin, javaScriptEnabled: false, storageState: await context.storageState() })
  t.after(() => plainContext.close())
  const plain = await plainContext.newPage()
  await plain.goto('/users/me/profile')
  await plain.getByLabel('Upload avatar').setInputFiles(path.join(repo, 'reference/test/fixtures/files/earth.png'))
  await plain.getByRole('button', { name: 'Save avatar', exact: true }).click()
  await plain.getByRole('button', { name: 'Remove avatar', exact: true }).waitFor()
})

test('channel unsaved changes allow cancel, discard, validation, save failure and save before leaving', { timeout: 120_000 }, async t => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 1311, height: 950 } })
  const id = roomPath.split('/').at(-1)
  const editPath = `/rooms/opens/${id}/edit`
  const modal = page.getByRole('dialog', { name: 'Unsaved Changes', exact: true })
  const admin = () => page.locator('nav.mb-rail a[href="/account/edit"]')
  await page.goto(editPath)
  const name = page.locator('#room_name')
  const original = await name.inputValue()
  await name.fill('A draft channel')
  await admin().click()
  await modal.waitFor()
  assert.equal(new URL(page.url()).pathname, editPath)
  assert.equal(await modal.getByRole('button', { name: 'Save', exact: true }).evaluate(node => node === document.activeElement), true)
  await capture(page, { path: path.join(artifacts, 'workspace-unsaved-changes.png'), fullPage: true })
  await page.keyboard.press('Escape')
  await modal.waitFor({ state: 'hidden' })
  assert.equal(await name.inputValue(), 'A draft channel')
  await admin().click()
  await modal.getByRole('button', { name: 'Discard', exact: true }).click()
  await page.waitForURL('**/account/edit')
  await page.goBack()
  await name.waitFor()
  assert.equal(await name.inputValue(), original, 'discarded changes never return from Turbo snapshots')

  await name.fill('')
  await admin().click()
  await modal.getByRole('button', { name: 'Save', exact: true }).click()
  await modal.waitFor({ state: 'hidden' })
  assert.equal(await name.evaluate(node => node.validity.valueMissing), true)
  assert.equal(new URL(page.url()).pathname, editPath)
  await name.fill('Saved from the dialog')
  await page.route(`**/rooms/opens/${id}`, route => route.fulfill({ status: 422, contentType: 'text/html', body: 'Rejected for this test' }))
  await admin().click()
  await modal.getByRole('button', { name: 'Save', exact: true }).click()
  await modal.getByRole('alert').getByText('Your changes couldn’t be saved. Please try again, or keep editing.', { exact: true }).waitFor()
  assert.equal(await name.inputValue(), 'Saved from the dialog')
  assert.equal(new URL(page.url()).pathname, editPath)
  await page.unroute(`**/rooms/opens/${id}`)
  let writes = 0
  page.on('request', request => { if (request.method() === 'POST' && new URL(request.url()).pathname === `/rooms/opens/${id}`) writes++ })
  await modal.getByRole('button', { name: 'Save', exact: true }).click()
  await page.waitForURL('**/account/edit')
  assert.equal(writes, 1)
  await page.goto(editPath)
  assert.equal(await name.inputValue(), 'Saved from the dialog')
  await name.fill('Changed then reverted')
  await name.fill('Saved from the dialog')
  await admin().click()
  await page.waitForURL('**/account/edit')
  assert.equal(await modal.count(), 0, 'reverting to the saved values does not prompt')
  const persisted = await context.request.get(editPath)
  assert.match(await persisted.text(), /value="Saved from the dialog"/)
})

test('channel guard retains access, icon and membership edits and protects browser history and mobile navigation', { timeout: 120_000 }, async t => {
  const { page, roomPath } = await setUp(t, { viewport: { width: 1311, height: 950 } })
  const id = roomPath.split('/').at(-1)
  const editPath = `/rooms/opens/${id}/edit`
  const modal = page.getByRole('dialog', { name: 'Unsaved Changes', exact: true })
  await page.locator(`#nav a[href="${editPath}"]`).click()
  await page.locator('#room_name').fill('Working draft')
  await page.locator('#room_icon').selectOption('coffee', { force: true })
  await page.locator('label[for="room_type"]').click()
  await page.waitForURL(`**/rooms/closeds/${id}/edit`)
  await page.locator(`form[data-ember-settings][action="/rooms/closeds/${id}"]`).waitFor()
  assert.equal(await page.locator('#room_name').inputValue(), 'Working draft')
  assert.equal(await page.locator('#room_icon').inputValue(), 'coffee')
  const member = page.locator('input[name="user_ids[]"]').first()
  await member.locator('..').click()
  assert.equal(await member.isChecked(), false)
  await page.locator('label[for="room_type"]').click()
  await page.waitForURL(`**${editPath}`)
  await page.locator(`form[data-ember-settings][action="/rooms/opens/${id}"]`).waitFor()
  await page.locator('label[for="room_type"]').click()
  await page.waitForURL(`**/rooms/closeds/${id}/edit`)
  await page.locator(`form[data-ember-settings][action="/rooms/closeds/${id}"]`).waitFor()
  assert.equal(await member.isChecked(), false, 'switching access forms preserves membership edits')
  await page.evaluate(() => history.back())
  await modal.waitFor()
  assert.equal(new URL(page.url()).pathname, `/rooms/closeds/${id}/edit`, 'canceling history restores the settings URL before prompting')
  await modal.getByRole('button', { name: 'Keep editing', exact: true }).click()
  assert.equal(await page.locator('#room_name').inputValue(), 'Working draft')
  await page.evaluate(() => history.back())
  await modal.waitFor()
  await modal.getByRole('button', { name: 'Discard', exact: true }).click()
  await page.waitForURL(`**${roomPath}`)
  await page.locator('#composer').waitFor()
  await page.goForward()
  await page.locator('#room_name').waitFor()
  assert.notEqual(await page.locator('#room_name').inputValue(), 'Working draft')
  await page.locator('#room_name').fill('Mobile draft')
  await page.setViewportSize({ width: 390, height: 844 })
  await page.getByRole('button', { name: 'Open conversations', exact: true }).click()
  await page.locator('nav.mb-rail a[href="/searches"]').click()
  await modal.waitFor()
  for (let step = 0; step < 4; step++) {
    await page.keyboard.press('Tab')
    assert.equal(await modal.evaluate(node => node.contains(document.activeElement)), true, 'keyboard focus stays in the dialog above the mobile drawer')
  }
  await noOverflow(page, 'mobile unsaved changes')
  await capture(page, { path: path.join(artifacts, 'workspace-unsaved-changes-mobile.png'), fullPage: true })
  await page.emulateMedia({ colorScheme: 'dark' })
  await capture(page, { path: path.join(artifacts, 'workspace-unsaved-changes-dark.png'), fullPage: true })
  await modal.getByRole('button', { name: 'Discard', exact: true }).click()
  await page.waitForURL('**/searches')
  assert.equal(await page.locator('body').evaluate(node => node.classList.contains('mb-sidebar-open')), false)
})

test('access-only edits and chosen icons are saved before following another destination', { timeout: 60_000 }, async t => {
  const { page, roomPath } = await setUp(t, { viewport: { width: 1311, height: 950 } })
  const id = roomPath.split('/').at(-1)
  await page.goto(`/rooms/opens/${id}/edit`)
  const original = await page.locator('#room_name').inputValue()
  await page.locator('label[for="room_type"]').click()
  await page.waitForURL(`**/rooms/closeds/${id}/edit`)
  await page.locator('nav.mb-rail a[href="/searches"]').click()
  const modal = page.getByRole('dialog', { name: 'Unsaved Changes', exact: true })
  await modal.waitFor()
  await page.keyboard.press('Escape')
  await page.locator('#room_icon').selectOption('coffee', { force: true })
  await page.locator('nav.mb-rail a[href="/searches"]').click()
  await modal.getByRole('button', { name: 'Save', exact: true }).click()
  await page.waitForURL('**/searches')
  await page.locator(`#shared_rooms a[href="${roomPath}"]`).click()
  await page.locator(`#nav a[href="/rooms/closeds/${id}/edit"]`).click()
  assert.equal(await page.locator('#room_name').inputValue(), original)
  assert.equal(await page.locator('#room_icon').inputValue(), 'coffee')
  assert.equal(await page.locator('#room_type').isChecked(), false)
})

test('native reload warns about channel drafts and cancel retains the form', { timeout: 60_000 }, async t => {
  const { page, roomPath } = await setUp(t)
  await page.goto(`/rooms/opens/${roomPath.split('/').at(-1)}/edit`)
  const original = await page.locator('#room_name').inputValue()
  await page.locator('#room_name').fill('Keep this draft')
  const warning = page.waitForEvent('dialog')
  const reload = page.reload().catch(() => {})
  const prompt = await warning
  assert.equal(prompt.type(), 'beforeunload')
  await prompt.dismiss()
  await reload
  assert.equal(await page.locator('#room_name').inputValue(), 'Keep this draft')
  await page.locator('#room_name').fill(original)
  await page.reload()
  assert.equal(await page.locator('#room_name').inputValue(), original)
})

test('save acknowledgment survives removing your own channel access and never treats sign-in as a save', { timeout: 90_000 }, async t => {
  const { page, context, server, joinURL } = await setUp(t, { viewport: { width: 1311, height: 950 } })
  const created = await context.request.post('/rooms/opens', { headers: sameOrigin, form: { 'room[name]': 'Hand over this channel' }, maxRedirects: 0 })
  assert.ok([302, 303].includes(created.status()), 'ordinary submissions keep their redirect')
  const roomPath = new URL(created.headers().location, server.origin).pathname
  const id = roomPath.split('/').at(-1)
  const { context: member } = await newPage(t, server)
  assert.equal((await member.request.post(joinURL, {
    headers: sameOrigin,
    multipart: { 'user[name]': 'Grace Hopper', 'user[email_address]': 'grace-handoff@example.test', 'user[password]': password },
  })).status(), 200)
  await page.goto(`/rooms/closeds/${id}/edit`)
  await page.getByRole('checkbox', { name: 'Give Ada Lovelace access to this room', exact: true }).locator('..').click()
  await page.locator('nav.mb-rail a[href="/searches"]').click()
  const modal = page.getByRole('dialog', { name: 'Unsaved Changes', exact: true })
  await page.route(`**/rooms/closeds/${id}`, route => route.fulfill({ status: 200, contentType: 'text/html', body: '<h1>Sign in</h1>' }))
  await modal.getByRole('button', { name: 'Save', exact: true }).click()
  await modal.getByRole('alert').waitFor()
  assert.equal(new URL(page.url()).pathname, `/rooms/closeds/${id}/edit`)
  await page.unroute(`**/rooms/closeds/${id}`)
  const saved = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === `/rooms/closeds/${id}`)
  await modal.getByRole('button', { name: 'Save', exact: true }).click()
  assert.equal((await saved).status(), 204)
  await page.waitForURL('**/searches')
  const memberView = await member.request.get(roomPath)
  assert.equal(new URL(memberView.url()).pathname, roomPath, 'the retained member can still open the channel')
  assert.match(await memberView.text(), /Hand over this channel/)
  await page.waitForFunction(path => !document.querySelector(`#shared_rooms a[href="${path}"]`), roomPath)
  assert.equal(await page.locator(`#shared_rooms a[href="${roomPath}"]`).count(), 0, 'the editor is no longer a member')
  const forbidden = await member.request.patch(`/rooms/closeds/${id}`, { headers: { ...sameOrigin, Prefer: 'return=minimal' }, form: { 'room[name]': 'Unauthorized change' } })
  assert.equal(forbidden.status(), 403, 'the minimal response never bypasses authorization')
})

test('DM picker searches full names and @names, keeps recipients, and handles mobile and failures', { timeout: 90_000 }, async t => {
  const { page, context, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1200, height: 900 } })
  await page.locator('#sidebar').getByRole('link', { name: 'New direct message', exact: true }).click()
  const emptyDialog = page.getByRole('dialog', { name: 'New direct message' })
  const emptySearch = emptyDialog.getByRole('combobox', { name: 'Search people' })
  await emptyDialog.getByText('No other people are available to message yet.', { exact: true }).waitFor()
  assert.equal(await emptySearch.inputValue(), '')
  assert.equal(await emptyDialog.getByText('No people found. Try another name.', { exact: true }).count(), 0)
  await emptySearch.fill('Nobody')
  await emptyDialog.getByText('No people found. Try another name.', { exact: true }).waitFor()
  for (const query of ['', '  ', '@']) {
    await emptySearch.fill(query)
    await emptyDialog.getByText('No other people are available to message yet.', { exact: true }).waitFor()
  }
  await capture(page, { path: path.join(artifacts, 'workspace-direct-message-empty.png') })
  await emptyDialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  for (const [index, name] of ['Grace Hopper', 'Maya & <Chen>'].entries()) {
    const { context: member } = await newPage(t, server)
    assert.equal((await member.request.post(joinURL, { headers: sameOrigin,
      multipart: { 'user[name]': name, 'user[email_address]': `dm-${index}@example.test`, 'user[password]': password },
    })).status(), 200)
  }
  const trigger = page.locator('#sidebar').getByRole('link', { name: 'New direct message', exact: true })
  await trigger.click()
  const dialog = page.getByRole('dialog', { name: 'New direct message' })
  const search = dialog.getByRole('combobox', { name: 'Search people' })
  await search.waitFor()
  assert.equal(await search.evaluate(node => node === document.activeElement), true)
  assert.equal(await dialog.getByRole('button', { name: 'Start conversation' }).isDisabled(), true)
  await dialog.getByRole('option', { name: 'Grace Hopper', exact: true }).waitFor()
  assert.equal(await dialog.getByRole('option', { name: 'Ada Lovelace', exact: true }).count(), 0)
  await search.fill('@Grace')
  await dialog.getByRole('option', { name: 'Grace Hopper', exact: true }).waitFor()
  await search.press('ArrowDown')
  await search.press('Enter')
  await dialog.getByRole('button', { name: 'Remove Grace Hopper' }).waitFor()
  await search.fill('Maya')
  await dialog.getByRole('option', { name: 'Maya & <Chen>', exact: true }).click()
  assert.equal(await dialog.locator('input[name="user_ids[]"]').count(), 2)
  await page.evaluate(async () => {
    const frame = document.querySelector('#user_sidebar')
    await frame.loaded
    frame.reload()
    await frame.loaded
  })
  assert.equal(await dialog.isVisible(), true)
  assert.equal(await dialog.locator('input[name="user_ids[]"]').count(), 2)
  await dialog.getByRole('button', { name: 'Remove Maya & <Chen>' }).click()
  await search.fill('Nobody matches this')
  await dialog.getByText('No people found. Try another name.', { exact: true }).waitFor()
  await page.route('**/autocompletable/users?*', route => route.fulfill({ status: 503 }))
  await search.fill('Grace')
  await dialog.getByText('Couldn’t load people. Please try again.', { exact: true }).waitFor()
  await page.unroute('**/autocompletable/users?*')
  await dialog.getByRole('button', { name: 'Try again' }).click()
  await dialog.getByText('Everyone shown is selected.', { exact: true }).waitFor()
  await capture(page, { path: path.join(artifacts, 'workspace-direct-message-dialog.png') })
  await search.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  assert.equal(await trigger.evaluate(node => node === document.activeElement), true)
  assert.equal(new URL(page.url()).pathname, roomPath)
  await page.locator('nav.mb-rail').getByRole('link', { name: 'DMs', exact: true }).click()
  await dialog.waitFor()
  assert.equal(await dialog.locator('input[name="user_ids[]"]').count(), 0)
  await search.fill('Grace')
  await dialog.getByRole('option', { name: 'Grace Hopper', exact: true }).click()
  await page.route('**/rooms/directs', route => route.fulfill({ status: 503, contentType: 'text/html', body: '<html><body>Service unavailable</body></html>' }))
  await dialog.getByRole('button', { name: 'Start conversation' }).click()
  await dialog.getByText('Couldn’t start the conversation. Please try again.', { exact: true }).waitFor()
  assert.equal(await dialog.getByRole('button', { name: 'Start conversation' }).isEnabled(), true)
  assert.equal(await dialog.locator('input[name="user_ids[]"]').count(), 1, 'HTML errors preserve the selected recipient')
  assert.equal(new URL(page.url()).pathname, roomPath, 'HTML errors do not replace the workspace')
  await page.unroute('**/rooms/directs')
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await page.setViewportSize({ width: 390, height: 844 })
  await page.emulateMedia({ colorScheme: 'dark' })
  await page.getByRole('button', { name: 'Open conversations', exact: true }).click()
  await trigger.click()
  await dialog.waitFor()
  assert.equal(await page.locator('body').evaluate(body => body.classList.contains('mb-sidebar-open')), false)
  await search.fill('@Grace Hopper')
  await dialog.getByRole('option', { name: 'Grace Hopper', exact: true }).click()
  const bounds = await dialog.boundingBox()
  assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 391 && bounds.y >= 0 && bounds.y + bounds.height <= 845)
  await noOverflow(page, 'mobile DM modal')
  await capture(page, { path: path.join(artifacts, 'workspace-direct-message-mobile-dark.png') })
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  assert.equal(await page.getByRole('button', { name: 'Open conversations', exact: true }).evaluate(node => node === document.activeElement), true)

  await page.setViewportSize({ width: 1200, height: 900 })
  await page.goto(`/rooms/opens/${roomPath.split('/').at(-1)}/edit`)
  await page.locator('#room_name').fill('Keep this draft')
  await page.locator('nav.mb-rail').getByRole('link', { name: 'DMs', exact: true }).click()
  await dialog.waitFor()
  await search.fill('Grace')
  await dialog.getByRole('option', { name: 'Grace Hopper', exact: true }).click()
  await dialog.getByRole('button', { name: 'Start conversation' }).click()
  const unsaved = page.getByRole('dialog', { name: 'Unsaved Changes', exact: true })
  await unsaved.waitFor()
  await unsaved.getByRole('button', { name: 'Keep editing', exact: true }).click()
  await unsaved.waitFor({ state: 'hidden' })
  assert.equal(await dialog.isVisible(), true)
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  assert.equal(await page.locator('#room_name').inputValue(), 'Keep this draft')
})

test('tabs left open across the Ember rename retain Activity and shared room-order updates', { timeout: 90_000 }, async t => {
  const { context: admin, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1200, height: 900 } })
  for (const name of ['Alpha', 'Zulu']) {
    assert.equal((await admin.request.post('/rooms/opens', { headers: sameOrigin, form: { 'room[name]': name } })).status(), 200)
  }
  const { page, context } = await newPage(t, server, { viewport: { width: 1200, height: 900 } })
  // Intercepted documents have no network address, so Chromium requires this for loopback Cable.
  await context.grantPermissions(['local-network-access'], { origin: server.origin })
  assert.equal((await context.request.post(joinURL, { headers: sameOrigin,
    multipart: { 'user[name]': 'Upgrade Member', 'user[email_address]': 'upgrade@example.test', 'user[password]': password },
  })).status(), 200)

  // Keep the actual previous shell and order controller in memory, just as an open tab does.
  // Only the initial document/frame gets legacy markup; later frames and Cable are untouched.
  const fixtures = path.join(repo, 'parity/kro/fixtures/pre-ember-rename')
  const [oldShell, oldOrderController] = await Promise.all([
    fs.readFile(path.join(fixtures, 'shell.js'), 'utf8'),
    fs.readFile(path.join(fixtures, 'channel_order_controller.js'), 'utf8'),
  ])
  const loadedFixtures = new Set()
  await page.route('**/assets/**', async route => {
    const pathname = new URL(route.request().url()).pathname
    if (/\/ember\/shell-[^/]+\.js$/.test(pathname)) {
      loadedFixtures.add('shell')
      return route.fulfill({ contentType: 'text/javascript', body: oldShell })
    }
    if (/\/controllers\/channel_order_controller-[^/]+\.js$/.test(pathname)) {
      loadedFixtures.add('order')
      return route.fulfill({ contentType: 'text/javascript', body: oldOrderController })
    }
    return route.continue()
  })
  let legacyMarkup = true
  const initialMarkup = async route => {
    if (!legacyMarkup) return route.continue()
    const response = await route.fetch()
    await route.fulfill({ response, body: (await response.text()).replaceAll('data-ember-', 'data-matchbox-') })
  }
  await page.route(`${server.origin}${roomPath}`, initialMarkup)
  await page.route(`${server.origin}/users/me/sidebar`, initialMarkup)
  await page.goto(roomPath)
  await page.locator('#shared_rooms a').nth(2).waitFor()
  await page.waitForFunction(() => [...document.querySelectorAll('turbo-cable-stream-source')].every(node => node.hasAttribute('connected')))
  assert.deepEqual([...loadedFixtures].sort(), ['order', 'shell'])
  assert.equal(await page.evaluate(() => window.matchboxWorkspace && !window.emberWorkspace), true, 'the previous shell is the only workspace runtime')
  assert.equal(await page.locator('#shared_rooms [data-ember-room-row]').count(), 0, 'the initial sidebar uses the previous hooks')
  const ids = await page.locator('#shared_rooms a').evaluateAll(rows => Object.fromEntries(rows.map(row => [row.dataset.sortedListName, Number(row.dataset.roomId)])))
  const postMessage = async name => {
    assert.equal((await admin.request.post(`/rooms/${ids[name]}/messages`, {
      headers: { ...sameOrigin, Accept: 'text/vnd.turbo-stream.html' }, form: { 'message[body]': `Upgrade unread in ${name}` },
    })).status(), 200)
  }
  const badge = page.locator('[data-matchbox-unread-count]')
  const activity = page.locator('[data-matchbox-activity]')
  const allTalk = page.locator('#shared_rooms a[data-sorted-list-name="All Talk"]')
  await postMessage('Alpha')
  await badge.filter({ hasText: '1' }).waitFor()
  await activity.click()
  assert.equal(await allTalk.isVisible(), false, 'the previous runtime filters read rooms before the upgrade')

  legacyMarkup = false
  const currentSidebar = page.waitForResponse(response => new URL(response.url()).pathname === '/users/me/sidebar')
  await page.evaluate(async () => {
    window.upgradeDocument = true
    const { cable } = await import('@hotwired/turbo-rails')
    const consumer = await cable.getConsumer()
    consumer.disconnect()
  })
  await page.waitForFunction(() => [...document.querySelectorAll('turbo-cable-stream-source')].every(node => !node.hasAttribute('connected')))
  await page.evaluate(async () => {
    const { cable } = await import('@hotwired/turbo-rails')
    ;(await cable.getConsumer()).connect()
  })
  assert.equal((await currentSidebar).status(), 200)
  await page.locator('#shared_rooms [data-ember-room-row]').nth(2).waitFor({ state: 'attached' })
  assert.equal(await allTalk.isVisible(), false, 'Activity still filters read rooms in the upgraded sidebar')
  assert.equal(await badge.isVisible(), true, 'the unread count survives the new sidebar markup')
  assert.equal(await badge.textContent(), '1')
  assert.equal(await activity.getAttribute('aria-pressed'), 'true')

  await postMessage('Zulu')
  await badge.filter({ hasText: '2' }).waitFor()
  assert.equal(await page.locator('#shared_rooms a:visible').count(), 2, 'new unread notifications still update Activity')
  const order = [ids.Zulu, ids['All Talk'], ids.Alpha]
  assert.equal((await admin.request.put('/account/room_order', { headers: sameOrigin, data: { room_ids: order } })).status(), 204)
  await page.waitForFunction(order => JSON.stringify([...document.querySelectorAll('#shared_rooms a')].map(row => Number(row.dataset.roomId))) === JSON.stringify(order), order)
  assert.equal(await allTalk.isVisible(), false, 'the order broadcast preserves the active unread filter')
  assert.equal(await badge.textContent(), '2')
  assert.equal(await page.evaluate(() => window.upgradeDocument && window.matchboxWorkspace && !window.emberWorkspace), true, 'no document reload or new workspace runtime was needed')
})

test('the Ember shell deduplicates legacy hooks and room-order stream aliases', { timeout: 90_000 }, async t => {
  const { context: admin, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1200, height: 900 } })
  for (const name of ['Alpha', 'Zulu']) {
    assert.equal((await admin.request.post('/rooms/opens', { headers: sameOrigin, form: { 'room[name]': name } })).status(), 200)
  }
  const { page, context } = await newPage(t, server, { viewport: { width: 1200, height: 900 } })
  assert.equal((await context.request.post(joinURL, { headers: sameOrigin,
    multipart: { 'user[name]': 'Current Member', 'user[email_address]': 'current-upgrade@example.test', 'user[password]': password },
  })).status(), 200)
  await page.goto(roomPath)
  await page.locator('#shared_rooms a').nth(2).waitFor()
  await page.waitForFunction(() => [...document.querySelectorAll('turbo-cable-stream-source')].every(node => node.hasAttribute('connected')))
  assert.equal(await page.evaluate(() => window.emberWorkspace && !window.matchboxWorkspace), true)
  const ids = await page.locator('#shared_rooms a').evaluateAll(rows => Object.fromEntries(rows.map(row => [row.dataset.sortedListName, Number(row.dataset.roomId)])))
  assert.equal((await admin.request.post(`/rooms/${ids.Alpha}/messages`, {
    headers: { ...sameOrigin, Accept: 'text/vnd.turbo-stream.html' }, form: { 'message[body]': 'Unread with both row hooks' },
  })).status(), 200)
  const badge = page.locator('[data-ember-unread-count]')
  await badge.filter({ hasText: '1' }).waitFor()
  assert.equal(await page.locator('#shared_rooms [data-ember-room-row][data-matchbox-room-row]').count(), 3)
  assert.equal(await badge.textContent(), '1', 'dual row hooks count each unread conversation once')
  await page.locator('[data-ember-activity]').click()
  assert.equal(await page.locator('#shared_rooms a:visible').count(), 1)
  await page.evaluate(() => {
    window.upgradeOrderEvents = 0
    window.addEventListener('ember:room-order-changed', () => window.upgradeOrderEvents++)
  })
  const order = [ids.Zulu, ids['All Talk'], ids.Alpha]
  assert.equal((await admin.request.put('/account/room_order', { headers: sameOrigin, data: { room_ids: order } })).status(), 204)
  await page.waitForFunction(order => JSON.stringify([...document.querySelectorAll('#shared_rooms a')].map(row => Number(row.dataset.roomId))) === JSON.stringify(order), order)
  assert.equal(await page.evaluate(() => window.upgradeOrderEvents), 1, 'one server broadcast emits one local refresh event')

  // A new shell can also encounter older cached/sidebar markup during a deployment.
  await page.route(`${server.origin}/users/me/sidebar`, async route => {
    const response = await route.fetch()
    await route.fulfill({ response, body: (await response.text()).replaceAll('data-ember-', 'data-matchbox-') })
  })
  for (const [index, action] of ['matchbox_room_order_changed', 'ember_room_order_changed'].entries()) {
    const refreshed = page.waitForResponse(response => new URL(response.url()).pathname === '/users/me/sidebar')
    await page.evaluate(action => window.Turbo.renderStreamMessage(`<turbo-stream action="${action}"></turbo-stream>`), action)
    assert.equal((await refreshed).status(), 200)
    await page.waitForFunction(() => document.querySelectorAll('#shared_rooms [data-ember-room-row]').length === 0)
    await page.evaluate(async () => { await document.querySelector('#user_sidebar').loaded })
    assert.equal(await page.evaluate(() => window.upgradeOrderEvents), index + 2, `${action} emits exactly one local refresh event`)
    assert.equal(await badge.textContent(), '1', 'legacy-only rows retain the unread count')
    assert.equal(await page.locator('#shared_rooms a:visible').count(), 1, 'legacy-only rows retain Activity filtering')
  }
})

test('admin settings stage the shared room order and preserve only personal starred ordering', { timeout: 90_000 }, async t => {
  const { page, context, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1200, height: 1000 } })
  for (const name of ['Alpha', 'Zulu']) {
    assert.equal((await context.request.post('/rooms/opens', { headers: sameOrigin, form: { 'room[name]': name } })).status(), 200)
  }
  const member = await newPage(t, server, { viewport: { width: 1200, height: 900 } })
  assert.equal((await member.context.request.post(joinURL, { headers: sameOrigin,
    multipart: { 'user[name]': 'Room Member', 'user[email_address]': 'room-order@example.test', 'user[password]': password },
  })).status(), 200)
  await member.page.goto(roomPath)
  await member.page.locator('#shared_rooms a').nth(2).waitFor()
  await member.page.waitForFunction(() => [...document.querySelectorAll('turbo-cable-stream-source')].every(node => node.hasAttribute('connected')))
  const names = p => p.locator('#shared_rooms a').evaluateAll(rows => rows.map(row => row.dataset.sortedListName))
  const waitNames = (p, expected) => p.waitForFunction(expected => JSON.stringify([...document.querySelectorAll('#shared_rooms a')].map(row => row.dataset.sortedListName)) === JSON.stringify(expected), expected)
  const alpha = member.page.locator('#shared_rooms a[data-sorted-list-name="Alpha"]')
  await alpha.focus()
  await member.page.keyboard.press('Alt+ArrowUp')
  assert.deepEqual(await names(member.page), ['All Talk', 'Alpha', 'Zulu'], 'unstarred rooms do not respond to personal reorder shortcuts')
  const ids = await member.page.locator('#shared_rooms a').evaluateAll(rows => rows.map(row => Number(row.dataset.roomId)))
  assert.equal((await member.context.request.put('/users/me/sidebar/order', { headers: sameOrigin, data: { room_ids: ids } })).status(), 403)
  assert.equal((await member.context.request.put('/account/room_order', { headers: sameOrigin, data: { room_ids: ids } })).status(), 403)

  await page.goto('/account/edit')
  const editor = page.locator('.mb-admin-room-order')
  const rows = editor.locator('[data-channel-order-target="room"]')
  const rowNames = () => rows.evaluateAll(rows => rows.map(row => row.dataset.sortedListName))
  assert.deepEqual(await rowNames(), ['All Talk', 'Alpha', 'Zulu'])
  const zulu = rows.filter({ hasText: 'Zulu' })
  const handle = zulu.getByRole('button', { name: 'Move Zulu', exact: true })
  const from = await handle.boundingBox()
  const top = await rows.first().boundingBox()
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2)
  await page.mouse.down()
  await zulu.locator('xpath=self::*[contains(@class,"mb-channel-dragging")]').waitFor()
  await page.mouse.move(top.x + 18, top.y + 2, { steps: 8 })
  await page.mouse.up()
  assert.deepEqual(await rowNames(), ['Zulu', 'All Talk', 'Alpha'])
  assert.deepEqual(await names(member.page), ['All Talk', 'Alpha', 'Zulu'], 'dragging only stages the order')
  await editor.getByRole('button', { name: 'Cancel', exact: true }).click()
  assert.deepEqual(await rowNames(), ['All Talk', 'Alpha', 'Zulu'])
  await handle.focus()
  await page.keyboard.press('Alt+ArrowUp')
  await page.route('**/account/room_order', route => route.fulfill({ status: 503 }))
  await editor.getByRole('button', { name: 'Save order', exact: true }).click()
  await editor.getByText('Couldn’t save room order. Please try again.', { exact: true }).waitFor()
  assert.deepEqual(await rowNames(), ['All Talk', 'Zulu', 'Alpha'], 'save failure retains the draft')
  await page.unroute('**/account/room_order')
  const saved = page.waitForResponse(response => response.request().method() === 'PUT' && new URL(response.url()).pathname === '/account/room_order')
  await editor.getByRole('button', { name: 'Save order', exact: true }).click()
  assert.equal((await saved).status(), 204)
  await editor.getByText('Room order saved for everyone.', { exact: true }).waitFor()
  await waitNames(member.page, ['All Talk', 'Zulu', 'Alpha'])
  await page.reload()
  assert.deepEqual(await rowNames(), ['All Talk', 'Zulu', 'Alpha'])
  const alphaId = ids[1]
  const zuluId = ids[2]
  for (const id of [alphaId, zuluId]) await member.context.request.put(`/rooms/${id}/favorite`, { headers: { ...sameOrigin, Accept: 'application/json' }, form: { favorite: 'true' } })
  await member.page.reload()
  await waitNames(member.page, ['Alpha', 'Zulu', 'All Talk'])
  const starredSaved = member.page.waitForResponse(response => response.request().method() === 'PUT' && new URL(response.url()).pathname === '/users/me/sidebar/order')
  await member.page.locator('#shared_rooms a[data-sorted-list-name="Zulu"]').focus()
  await member.page.keyboard.press('Alt+ArrowUp')
  assert.equal((await starredSaved).status(), 204)
  await waitNames(member.page, ['Zulu', 'Alpha', 'All Talk'])
  await rows.filter({ hasText: 'Alpha' }).focus()
  await page.keyboard.press('Alt+ArrowUp')
  await editor.getByRole('button', { name: 'Save order', exact: true }).click()
  await waitNames(member.page, ['Zulu', 'Alpha', 'All Talk'])
  assert.equal((await context.request.put('/account/room_order', { headers: sameOrigin, data: { room_ids: [999999] } })).status(), 403)
  assert.equal((await context.request.put('/account/room_order', { headers: { 'Sec-Fetch-Site': 'cross-site' }, data: { room_ids: ids } })).status(), 422)
  await page.setViewportSize({ width: 390, height: 844 })
  await editor.scrollIntoViewIfNeeded()
  await noOverflow(page, 'mobile admin room ordering')
  await capture(page, { path: path.join(artifacts, 'workspace-admin-room-order.png'), fullPage: true })
  await member.page.goto('/account/edit')
  assert.equal(await member.page.locator('.mb-admin-room-order').count(), 0)
})

test('custom reactions use a spaced form with validation, cancel, and mobile layouts', { timeout: 90_000 }, async t => {
  const { page, roomPath } = await setUp(t, { viewport: { width: 1008, height: 797 } })
  await submitMessage(page, roomPath, 'First in this group.')
  const message = await submitMessage(page, roomPath, 'A short follow-up.')
  await page.waitForFunction(() => [...document.querySelectorAll('.message--threaded')].some(node => node.textContent.includes('A short follow-up.')))
  const card = message.locator('.mb-custom-reaction')
  const input = card.getByRole('textbox', { name: 'Custom reaction', exact: true })
  const open = async () => {
    await page.locator('#nav').hover()
    await message.hover()
    const menu = message.getByRole('group', { name: 'Message actions', exact: true })
    await menu.getByRole('button', { name: 'More reactions', exact: true }).click()
    await menu.getByRole('link', { name: 'New boost', exact: true }).click()
    await input.waitFor()
    await page.waitForFunction(() => document.activeElement?.name === 'boost[content]')
  }
  await open()
  assert.equal(await input.getAttribute('maxlength'), '16')
  await card.getByRole('button', { name: 'Add reaction', exact: true }).click()
  assert.equal(await input.evaluate(field => field.validity.valueMissing), true)
  await input.fill('  ')
  assert.equal(await input.evaluate(field => field.validity.patternMismatch), true)
  await input.fill('Lovely!')
  const fieldBox = await input.boundingBox()
  const addBox = await card.getByRole('button', { name: 'Add reaction', exact: true }).boundingBox()
  assert.ok(fieldBox.width >= 240 && fieldBox.height >= 40, `roomy input: ${JSON.stringify(fieldBox)}`)
  assert.ok(addBox.y >= fieldBox.y + fieldBox.height + 10, 'actions have their own row below the input')
  await noOverflow(page, 'desktop custom reaction')
  await capture(page, { path: path.join(artifacts, 'workspace-custom-reaction-desktop.png') })
  await card.getByRole('link', { name: 'Cancel', exact: true }).click()
  await card.waitFor({ state: 'detached' })
  await open()
  await input.press('Escape')
  await card.waitFor({ state: 'detached' })

  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 })
    await page.emulateMedia({ colorScheme: 'dark' })
    await open()
    const bounds = await card.boundingBox()
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width, `custom reaction fits ${width}px: ${JSON.stringify(bounds)}`)
    const body = await message.locator('[data-reply-target="body"]').boundingBox()
    assert.ok(body.width > 100 && body.height < 70, "the reaction form does not squeeze the message into a narrow column")
    const controls = await card.locator('input, .btn').evaluateAll(nodes => nodes.map(node => {
      const box = node.getBoundingClientRect()
      return { left: box.left, right: box.right, width: box.width }
    }))
    assert.ok(controls.every(box => box.width > 45 && box.left >= bounds.x && box.right <= bounds.x + bounds.width), 'every control fits inside its card')
    await noOverflow(page, 'mobile custom reaction')
    await input.fill('Lovely!')
    await capture(page, { path: path.join(artifacts, `workspace-custom-reaction-${width}-dark.png`) })
    if (width === 390) {
      await card.getByRole('link', { name: 'Cancel', exact: true }).click()
      await card.waitFor({ state: 'detached' })
    } else {
      const submitted = page.waitForResponse(response => response.request().method() === 'POST' && /\/boosts$/.test(new URL(response.url()).pathname))
      await card.getByRole('button', { name: 'Add reaction', exact: true }).click()
      assert.ok([302, 303].includes((await submitted).status()))
      await card.waitFor({ state: 'detached' })
      await message.locator('.boost-item').filter({ hasText: 'Lovely!' }).waitFor()
    }
  }
})


test('inline Add a boost opens the shared emoji tray and survives reaction-frame replacement', { timeout: 90_000 }, async t => {
  const { page, context, roomPath, server } = await setUp(t, { viewport: { width: 1200, height: 900 } })
  const message = await submitMessage(page, roomPath, 'React from either control.')
  await message.hover()
  await message.getByRole('button', { name: 'Thumbs up', exact: true }).click()
  await message.locator('.boost-item').first().waitFor()
  const inline = message.getByRole('link', { name: 'Add a boost', exact: true })
  const tray = message.locator('.mb-message-reaction-tray')
  await page.locator('#nav').hover()
  await inline.focus()
  await page.keyboard.press('Enter')
  await tray.waitFor()
  assert.equal(await message.locator('.input--boost').count(), 0, 'the inline action opens emoji choices, not the custom-text form')
  assert.equal(await tray.evaluate(node => node.contains(document.activeElement)), true)
  await page.keyboard.press('Escape')
  await tray.waitFor({ state: 'hidden' })
  assert.equal(await inline.evaluate(node => node === document.activeElement), true)
  await inline.click()
  await tray.getByRole('button', { name: 'Fire', exact: true }).click()
  await message.locator('.boost-item').filter({ hasText: '🔥' }).waitFor()
  assert.equal(await message.locator('[data-message-actions-target="trigger"]').evaluate(node => node === document.activeElement), true, 'submission keeps focus on a control that survives the response')
  await inline.click()
  await tray.waitFor()
  await tray.getByRole('link', { name: 'New boost', exact: true }).click()
  await message.getByRole('textbox', { name: 'Custom reaction', exact: true }).waitFor()
  await message.getByRole('link', { name: 'Cancel', exact: true }).click()
  await inline.waitFor()
  await inline.click()
  await tray.waitFor()
  await page.locator('#nav').click({ position: { x: 120, y: 20 } })
  await tray.waitFor({ state: 'hidden' })
  const mobile = await newPage(t, server, { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, storageState: await context.storageState() })
  await mobile.page.goto(roomPath)
  await mobile.page.getByRole('link', { name: 'Add a boost', exact: true }).tap()
  await mobile.page.locator('.mb-message-reaction-tray').waitFor()
  await noOverflow(mobile.page, 'inline mobile emoji picker')
  await capture(mobile.page, { path: path.join(artifacts, 'workspace-inline-emoji-picker.png') })
})
