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
const binary = path.resolve(process.env.MATCHBOX_BIN || path.join(repo, "target/debug/matchbox"))
const artifacts = path.join(repo, "parity/out/kro")
const password = "matchbox-workspace-test-password"
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
  await page.evaluate(async () => {
    const animations = document.getAnimations().filter((animation) => Number.isFinite(animation.effect?.getComputedTiming().endTime))
    await Promise.all(animations.map((animation) => animation.finished.catch(() => {})))
    await Promise.all([...document.images].filter((image) => {
      const bounds = image.getBoundingClientRect()
      return bounds.width && bounds.height && bounds.bottom > 0 && bounds.top < innerHeight && bounds.right > 0 && bounds.left < innerWidth
    }).map((image) => image.decode().catch(() => {})))
    await new Promise((resolve) => requestAnimationFrame(resolve))
  })
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
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "matchbox-workspace-browser-"))
  const port = await freePort()
  const targetPort = await freePort(port)
  const origin = `http://127.0.0.1:${port}`
  const lifecycle = { closing: false }
  const env = { ...process.env }
  // Each test owns its complete installation; never inherit the user's running app or data.
  for (const key of Object.keys(env)) {
    if (/^(MATCHBOX_|CAMPFIRE_|THRUSTER_|VAPID_)/.test(key)) delete env[key]
  }
  Object.assign(env, {
    SECRET_KEY_BASE: "workspace-disposable-browser-test-secret".repeat(4),
    MATCHBOX_STORAGE_PATH: dir,
    MATCHBOX_DATABASE_PATH: path.join(dir, "db/production.sqlite3"),
    MATCHBOX_FILES_PATH: path.join(dir, "files"),
    MATCHBOX_BACKUPS_PATH: path.join(dir, "backups"),
    MATCHBOX_LOG: "error",
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
    if (child.exitCode !== null) throw new Error(`Matchbox exited: ${log}`)
    try {
      const response = await fetch(`${origin}/first_run`, { signal: AbortSignal.timeout(1000) })
      if (response.ok) return { origin, dir, lifecycle }
    } catch { /* The listening sockets are not ready yet. */ }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`Matchbox did not become ready: ${log}`)
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
  await page.locator("#shared_rooms a").first().waitFor()
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
  assert.equal(sheets.filter((href) => href.includes("zz-matchbox")).length, 1, "workspace stylesheet loads once")
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

test("workspace navigation, conversation filtering, and room creation use the real application", { timeout: 90_000 }, async (t) => {
  const { page, context, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1440, height: 1000 } })
  await shell(page)
  const boxes = await page.evaluate(() => ["nav.mb-rail", "aside#sidebar", "#main-content"].map((selector) => {
    const node = document.querySelector(selector)
    const { x, width } = node.getBoundingClientRect()
    return { x, width }
  }))
  assert.ok(boxes[0].x < boxes[1].x && boxes[1].x < boxes[2].x, "rail, conversations, and content are arranged left to right")
  await page.locator("nav.mb-rail [data-matchbox-activity]").click()
  assert.equal(await page.locator("[data-matchbox-activity]").getAttribute("aria-pressed"), "true")
  await page.locator("[data-matchbox-room-empty]").getByText("You’re all caught up. No unread conversations.", { exact: true }).waitFor()
  await page.locator("nav.mb-rail [data-matchbox-activity]").click()
  await page.locator('#sidebar a[href="/rooms/opens/new"]').click()
  await page.locator("#room_name").fill("Design")
  await page.getByRole("button", { name: "Save", exact: true }).click()
  await page.locator("#composer").waitFor()
  const designPath = new URL(page.url()).pathname
  assert.notEqual(designPath, roomPath)
  const filter = page.locator("[data-matchbox-room-filter]")
  await filter.fill("design")
  assert.equal(await page.locator(`#shared_rooms a[href="${designPath}"]`).isVisible(), true)
  assert.equal(await page.locator(`#shared_rooms a[href="${roomPath}"]`).isVisible(), false)
  await filter.fill("no such conversation")
  assert.equal(await page.locator("#shared_rooms a:visible").count(), 0)
  await filter.fill("")
  await page.locator(`#shared_rooms a[href="${roomPath}"]`).click()
  await page.waitForURL(`**${roomPath}`)
  await page.waitForFunction((path) => document.querySelector(`#shared_rooms a[href="${path}"]`)?.getAttribute("aria-current") === "page", roomPath)
  await page.getByText("Welcome to Matchbox", { exact: true }).waitFor()

  const { context: member } = await newPage(t, server)
  const joined = await member.request.post(joinURL, {
    headers: sameOrigin,
    multipart: { "user[name]": "Maya Chen", "user[email_address]": "maya@example.test", "user[password]": password },
  })
  assert.equal(joined.status(), 200)
  await page.locator('#sidebar a[href="/rooms/directs/new"]').first().click()
  await page.locator("#rooms_direct_user_ids_input").fill("Maya")
  await page.locator("[role=option]").filter({ hasText: "Maya Chen" }).click()
  await page.waitForFunction(() => document.querySelector('#direct_rooms_control select[name="user_ids[]"]')?.selectedOptions.length > 0)
  const directForm = await page.getByRole("button", { name: "Start Ping", exact: true }).evaluate((button) => ({
    form: button.form?.action,
    valid: button.form?.checkValidity(),
    values: [...(button.form?.elements || [])].map((element) => ({ name: element.name, value: element.value, valid: element.validity?.valid, disabled: element.disabled })),
  }))
  assert.ok(directForm.form && directForm.valid, `direct message form must be valid: ${JSON.stringify(directForm)}`)
  await page.getByRole("button", { name: "Start Ping", exact: true }).click()
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
  await page.locator("#account_name").fill("Matchbox Studio")
  const accountForm = page.locator("form").filter({ has: page.locator("#account_name") })
  const accountSaved = page.waitForResponse((response) => response.request().method() === "POST" && /^\/account(?:\.|$)/.test(new URL(response.url()).pathname))
  await accountForm.getByRole("button", { name: "Save changes", exact: true }).click()
  assert.ok([302, 303].includes((await accountSaved).status()))
  await page.locator("#account_name").waitFor()
  await page.reload()
  assert.equal(await page.locator("#account_name").inputValue(), "Matchbox Studio")
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
  await message.locator('lexxy-editor [contenteditable="true"]').fill("Every detail has a place in Matchbox.")
  const saveForm = await message.getByRole("button", { name: "Save changes", exact: true }).evaluate((button) => ({
    form: button.form?.id,
    valid: button.form?.checkValidity(),
    invalid: [...(button.form?.elements || [])].filter((element) => !element.validity?.valid).map((element) => ({ name: element.name, validation: element.validationMessage })),
  }))
  assert.ok(saveForm.form && saveForm.valid, `message editor must have a valid connected form: ${JSON.stringify(saveForm)}`)
  const saved = page.waitForResponse((response) => response.request().method() === "POST" && /\/messages\/\d+$/.test(new URL(response.url()).pathname))
  await message.getByRole("button", { name: "Save changes", exact: true }).click()
  assert.ok([302, 303].includes((await saved).status()))
  const edited = page.locator(".message[data-message-id]").filter({ hasText: "Every detail has a place in Matchbox." })
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
  await page.locator("#search-results").getByText("Every detail has a place in Matchbox.", { exact: true }).waitFor()
  await shell(page)
  await noOverflow(page, "desktop search")
  await capture(page, { path: path.join(artifacts, "workspace-search.png"), fullPage: true })
  await page.goBack()
  await page.goBack()
  await page.locator("#composer").waitFor()
  await submitMessage(page, roomPath, "Back navigation keeps the composer working.")
  assert.equal(await page.locator("nav.mb-rail").count(), 1)
  assert.equal(await page.locator("[data-matchbox-room-filter]").count(), 1)
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
      const toggle = page.locator("[data-matchbox-sidebar-toggle]").filter({ visible: true }).first()
      await toggle.waitFor()
      assert.equal(await toggle.getAttribute("aria-expanded"), "false")
      await toggle.click()
      await page.waitForFunction(() => document.querySelector('[data-matchbox-sidebar-toggle][aria-expanded="true"]'))
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
      await page.locator(".mb-sidebar-close[data-matchbox-sidebar-close]").click()
      assert.equal(await toggle.getAttribute("aria-expanded"), "false")
      await toggle.click()
      await page.locator(".mb-sidebar-backdrop[data-matchbox-sidebar-close]").click({ position: { x: width - 4, y: 420 } })
      assert.equal(await toggle.getAttribute("aria-expanded"), "false", "backdrop closes the drawer")
      await toggle.click()
      await page.locator(`#shared_rooms a[href="${roomPath}"]`).click()
      await page.waitForFunction(() => !document.querySelector('[data-matchbox-sidebar-toggle][aria-expanded="true"]'))
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

test("the workspace follows system appearance and public forms remain responsive", { timeout: 60_000 }, async (t) => {
  const { server, page, roomPath, joinURL } = await setUp(t, { viewport: { width: 1280, height: 900 } })
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
  const { page } = await setUp(t, { viewport: { width: 1311, height: 1152 } })
  await page.waitForLoadState("networkidle")
  const search = page.locator('[data-matchbox-room-filter]')
  await search.focus()
  const searchFocus = await search.evaluate((input) => ({
    inputOutline: getComputedStyle(input).outlineStyle,
    outerOutline: getComputedStyle(input.closest('label')).outlineWidth,
    outerRadius: getComputedStyle(input.closest('label')).borderRadius,
  }))
  assert.equal(searchFocus.inputOutline, 'none', 'only the rounded search container shows focus')
  assert.equal(searchFocus.outerOutline, '2px')
  assert.notEqual(searchFocus.outerRadius, '0px')

  await page.locator('#sidebar a[href="/rooms/opens/new"]').click()
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

  await page.goto('/account/edit')
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
  await page.setViewportSize({ width: 390, height: 844 })
  await noOverflow(page, 'mobile polished settings')
  await capture(page, { path: path.join(artifacts, 'workspace-polished-settings-mobile.png'), fullPage: true })
})

test("SVG attachments preview as isolated images and keep safe downloads", { timeout: 60_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 1311, height: 900 } })
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180" viewBox="0 0 320 180" onload="parent.svgExecuted=true"><script>parent.svgExecuted=true;fetch('/svg-preview-executed')</script><image href="https://svg-preview.example.test/tracker.png" width="1" height="1"/><rect width="320" height="180" fill="#336699"/><text x="20" y="90" fill="white">Matchbox SVG</text></svg>`
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

test("personal channel ordering persists, remains isolated, and can reset", { timeout: 90_000 }, async (t) => {
  const { page, context, roomPath, joinURL, server } = await setUp(t, { viewport: { width: 1311, height: 900 } })
  for (const name of ["Zulu", "Alpha"]) {
    const response = await context.request.post("/rooms/opens", { headers: sameOrigin, form: { "room[name]": name } })
    assert.equal(response.status(), 200)
  }
  await page.reload()
  const names = () => page.locator('#shared_rooms [data-channel-order-target="room"]').evaluateAll((rows) => rows.map((row) => row.dataset.sortedListName))
  await page.waitForFunction(() => document.querySelectorAll('#shared_rooms [data-channel-order-target="room"]').length === 3)
  const alphabetic = await names()
  assert.deepEqual(alphabetic, ["All Talk", "Alpha", "Zulu"])
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
  await page.getByText("Couldn’t save channel order. Please try again.", { exact: true }).waitFor()
  assert.deepEqual(await names(), ["All Talk", "Zulu", "Alpha"], "failed saves restore the last saved order")
  await context.unroute('**/users/me/sidebar/order')
  const reset = page.waitForResponse((response) => response.request().method() === "PUT" && new URL(response.url()).pathname === "/users/me/sidebar/order")
  await page.getByRole("button", { name: "Reset channels to alphabetical order" }).click()
  assert.equal((await reset).status(), 204)
  assert.deepEqual(await names(), alphabetic)
  await page.reload()
  await zulu.waitFor()
  assert.deepEqual(await names(), alphabetic)
  assert.equal(await page.getByRole("button", { name: "Reset channels to alphabetical order" }).count(), 0)
})

test('mobile channel ordering supports real touch holds and cancellation', { timeout: 60_000 }, async (t) => {
  const { context, roomPath, server } = await setUp(t)
  for (const name of ['Zulu', 'Alpha']) {
    const response = await context.request.post('/rooms/opens', { headers: sameOrigin, form: { 'room[name]': name } })
    assert.equal(response.status(), 200)
  }
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
  await phone.getByText('Channel move canceled.', { exact: true }).waitFor({ state: 'attached' })
  assert.deepEqual(await phoneNames(), ['Zulu', 'All Talk', 'Alpha'], 'touch cancellation restores the saved order')
  assert.equal(cancelWrites, 0, 'canceled touch reorder never writes a preference')
  await phone.reload()
  await phone.getByRole('button', { name: 'Open conversations', exact: true }).click()
  await phoneZulu.waitFor()
  assert.deepEqual(await phoneNames(), ['Zulu', 'All Talk', 'Alpha'])
  await noOverflow(phone, 'mobile channel ordering')
  await capture(phone, { path: path.join(artifacts, 'workspace-channel-ordering-mobile.png'), fullPage: true })
})

test('channel ordering scrolls long lists and updates the drop position at a stationary edge', { timeout: 60_000 }, async (t) => {
  const { page, context, roomPath } = await setUp(t, { viewport: { width: 1311, height: 500 } })
  for (let index = 1; index <= 24; index++) {
    const response = await context.request.post('/rooms/opens', {
      headers: sameOrigin, form: { 'room[name]': `Channel ${String(index).padStart(2, '0')}` },
    })
    assert.equal(response.status(), 200)
  }
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
  assert.match(await page.locator("#footer").innerText(), /Matchbox™ version 1\.0/)
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
    await imageMessage.scrollIntoViewIfNeeded()
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
  const launcher = page.getByRole('button', { name: 'Choose channel icon', exact: true })
  const dialog = page.getByRole('dialog', { name: 'Choose a channel icon', exact: true })
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
  assert.equal(await memberPage.getByRole('button', { name: 'Choose channel icon', exact: true }).count(), 0, 'members cannot open an unauthorized editor')
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
  const launcher = page.getByRole('button', { name: 'Choose channel icon', exact: true })
  const dialog = page.getByRole('dialog', { name: 'Choose a channel icon', exact: true })
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
