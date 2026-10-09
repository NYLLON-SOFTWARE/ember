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
const password = "kro-browser-test-password"
let browser

before(async () => {
  await fs.access(binary)
  await fs.mkdir(artifacts, { recursive: true })
  browser = await chromium.launch()
})
after(async () => { await browser?.close() })

async function freePort(except) {
  const listener = net.createServer()
  listener.listen(0, "127.0.0.1")
  await once(listener, "listening")
  const port = listener.address().port
  await new Promise((resolve) => listener.close(resolve))
  return port === except ? freePort(except) : port
}

async function freshServer(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "matchbox-kro-browser-"))
  const port = await freePort()
  const targetPort = await freePort(port)
  const origin = `http://127.0.0.1:${port}`
  const env = { ...process.env }
  // Never inherit the developer's database paths, TLS domains, fixed clock, or front-server ports.
  for (const key of Object.keys(env)) {
    if (/^(MATCHBOX_|CAMPFIRE_|THRUSTER_|VAPID_)/.test(key)) delete env[key]
  }
  Object.assign(env, {
    SECRET_KEY_BASE: "kro-disposable-browser-test-secret".repeat(4),
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
    if (child.exitCode === null && !spawnError) {
      const exited = once(child, "exit")
      child.kill("SIGTERM")
      const timer = setTimeout(() => child.kill("SIGKILL"), 5000)
      await exited
      clearTimeout(timer)
    }
    if (log) await fs.writeFile(path.join(artifacts, `${port}.server.log`), log)
    await fs.rm(dir, { recursive: true, force: true })
  })
  const deadline = Date.now() + 20_000
  while (Date.now() < deadline) {
    if (spawnError) throw spawnError
    if (child.exitCode !== null) throw new Error(`Matchbox exited: ${log}`)
    try {
      const response = await fetch(`${origin}/first_run`, { signal: AbortSignal.timeout(1000) })
      if (response.ok) return { origin, dir }
    } catch { /* The listening sockets are not ready yet. */ }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`Matchbox did not become ready: ${log}`)
}

async function newPage(t, server, options = {}) {
  const context = await browser.newContext({ baseURL: server.origin, ...options })
  t.after(() => context.close())
  const page = await context.newPage()
  page.setDefaultTimeout(10_000)
  const errors = []
  page.on("pageerror", (error) => errors.push(error.message))
  t.after(() => assert.deepEqual(errors, [], "browser JavaScript errors"))
  return { page, context }
}

async function profile(page, expected) {
  if (expected === "basecoat") {
    await page.locator('body[data-style-profile="basecoat"]').waitFor()
  } else {
    assert.equal(await page.locator('body[data-style-profile="basecoat"]').count(), 0)
  }
  const stylesheets = await page.locator('link[rel="stylesheet"]').evaluateAll((links) => links.map((link) => link.href))
  assert.ok(stylesheets.length > 0, "page has stylesheets")
  const basecoat = stylesheets.filter((href) => href.includes("basecoat"))
  if (expected === "basecoat") {
    assert.equal(basecoat.length, 1, "Basecoat page loads exactly one generated stylesheet")
    assert.equal(stylesheets.length, 1, "Basecoat page does not load legacy stylesheets")
  } else {
    assert.equal(basecoat.length, 0, "legacy page has no Basecoat stylesheet")
  }
}

async function fillSignup(page, email) {
  await page.locator("#user_name").fill("Ada Browser")
  await page.locator("#user_email_address").fill(email)
  await page.locator("#user_password").fill(password)
}

async function submitSignup(page) {
  const posted = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === "/first_run")
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  const response = await posted
  assert.ok([302, 303].includes(response.status()), "setup POST redirects after creating the account")
  assert.match(response.request().headers()["content-type"], /^multipart\/form-data;/)
  await page.locator("#message-area").waitFor()
  assert.notEqual(new URL(page.url()).pathname, "/first_run")
  await profile(page, "legacy")
}

async function expectDark(page, dark) {
  await page.waitForFunction((expected) => document.documentElement.classList.contains("dark") === expected, dark)
}

test("administrators control translation visibility for everyone, hidden by default", { timeout: 60_000 }, async (t) => {
  const server = await freshServer(t)
  const { page, context } = await newPage(t, server)
  await page.goto("/first_run")
  await fillSignup(page, "owner@example.test")
  await submitSignup(page)
  const roomPath = new URL(page.url()).pathname
  const translationMenus = ".language-list-menu"
  assert.equal(await page.locator(translationMenus).count(), 0, "welcome translations are hidden by default")
  await page.goto("/account/edit")
  const toggle = page.getByRole("checkbox", { name: "Hide translation buttons", exact: true })
  assert.equal(await toggle.isChecked(), true)
  const accountAction = await page.locator("#localization-settings form").getAttribute("action")
  const joinURL = await page.locator("#invite_url").inputValue()
  await page.screenshot({ path: path.join(artifacts, "localization-settings.png"), fullPage: true })
  const { page: guest, context: guestContext } = await newPage(t, server)
  for (const url of ["/session/new", joinURL]) {
    await guest.goto(url)
    assert.equal(await guest.locator(translationMenus).count(), 0, `translations hidden on ${new URL(guest.url()).pathname}`)
  }

  async function setHidden(hidden) {
    await page.goto("/account/edit")
    await page.evaluate(() => { window.matchboxVisibilityMarker = true })
    const saved = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === accountAction)
    // The existing switch styles deliberately hide the native checkbox behind its label.
    await page.locator("#localization-settings label.switch").click()
    assert.ok([302, 303].includes((await saved).status()))
    await page.waitForFunction(() => !window.matchboxVisibilityMarker)
    assert.equal(await toggle.isChecked(), hidden, "changed tracked metadata forces a fresh document")
    await page.reload()
    assert.equal(await toggle.isChecked(), hidden, "setting persists after reload")
    assert.equal(await page.locator(translationMenus).count() > 0, !hidden)
    for (const url of [roomPath, "/users/me/profile"]) {
      await page.goto(url)
      assert.equal(await page.locator(translationMenus).count() > 0, !hidden, `authenticated visibility on ${url}`)
    }
    for (const url of ["/session/new", joinURL]) {
      await guest.goto(url)
      assert.equal(await guest.locator(translationMenus).count() > 0, !hidden, `public visibility on ${new URL(guest.url()).pathname}`)
    }
  }
  await setHidden(false)

  const { context: memberContext, page: member } = await newPage(t, server)
  const joined = await memberContext.request.post(joinURL, {
    headers: { "Sec-Fetch-Site": "same-origin" },
    multipart: { "user[name]": "Member", "user[email_address]": "member@example.test", "user[password]": password },
  })
  assert.equal(joined.status(), 200)
  await member.goto("/account/edit")
  assert.equal(await member.locator("#localization-settings").count(), 0, "members cannot see the administrator control")
  const denied = await memberContext.request.post(accountAction, {
    headers: { "Sec-Fetch-Site": "same-origin" },
    form: { _method: "put", "account[settings][hide_translation_buttons]": "true" },
    maxRedirects: 0,
  })
  assert.equal(denied.status(), 403, "members cannot change the setting via a direct request")
  const signedOut = await guestContext.request.post(accountAction, {
    headers: { "Sec-Fetch-Site": "same-origin" },
    form: { _method: "put", "account[settings][hide_translation_buttons]": "true" },
    maxRedirects: 0,
  })
  assert.equal(signedOut.status(), 302)
  assert.match(signedOut.headers().location, /\/session\/new$/)
  await page.goto("/account/edit")
  assert.equal(await toggle.isChecked(), false, "unauthorized requests did not change the setting")
  await setHidden(true)
  await member.reload()
  assert.equal(await member.locator(translationMenus).count(), 0)
  assert.equal((await context.request.get(roomPath)).status(), 200)
})

test("setup creates an account, switches style profiles, signs in, and prevents repeated setup", { timeout: 60_000 }, async (t) => {
  const server = await freshServer(t)
  const { page, context } = await newPage(t, server)
  await page.goto("/session/new")
  assert.equal(new URL(page.url()).pathname, "/first_run", "empty installation redirects sign-in to setup")
  await profile(page, "basecoat")
  assert.equal(await page.title(), "Set up Matchbox")
  const setupHTML = await (await context.request.get("/first_run")).text()
  await page.evaluate(() => { window.kroDocumentMarker = "setup" })
  await fillSignup(page, "ada@example.test")
  await submitSignup(page)
  await page.getByText("Welcome to Matchbox", { exact: true }).waitFor()
  const manifest = await (await context.request.get("/webmanifest.json")).json()
  assert.equal(manifest.name, "Matchbox")
  assert.ok(!JSON.stringify(manifest).includes("Campfire"))
  assert.ok(!await page.locator("body").innerText().then(text => text.includes("Campfire")))
  assert.ok((await (await context.request.get("/502.html")).text()).includes("Starting Matchbox"))
  assert.equal(await page.evaluate(() => window.kroDocumentMarker), undefined, "cross-profile signup creates a new document")
  // MessagesController#index uses fresh_when for non-empty pages; a new install's empty page is
  // deliberately 204, so create a message before checking the unchanged cache contract.
  const messagesPath = `${new URL(page.url()).pathname}/messages`
  const postMessage = (body) => context.request.post(messagesPath, {
    headers: { Accept: "text/vnd.turbo-stream.html", "Sec-Fetch-Site": "same-origin" },
    form: { "message[body]": `<div>${body}</div>` },
  })
  assert.equal((await postMessage("Campfire is the upstream project")).status(), 200)
  const firstMessages = await context.request.get(messagesPath)
  const warmMessages = await context.request.get(messagesPath)
  assert.equal(firstMessages.status(), 200)
  assert.match(await firstMessages.text(), /Campfire is the upstream project/, "branding must not rewrite chat contents")
  assert.equal(warmMessages.status(), 200)
  const etag = firstMessages.headers().etag
  assert.match(etag, /^W\/".+"$/, "legacy message pages retain their weak ETag")
  assert.equal(warmMessages.headers().etag, etag, "message ETag is stable across repeated rendering")
  assert.equal(await firstMessages.text(), await warmMessages.text(), "cached and repeated message HTML agree")
  const unchanged = await context.request.get(messagesPath, { headers: { "If-None-Match": etag } })
  assert.equal(unchanged.status(), 304)
  assert.equal(await unchanged.text(), "")
  assert.equal((await postMessage("KRO cache invalidation")).status(), 200)
  const changed = await context.request.get(messagesPath, { headers: { "If-None-Match": etag } })
  assert.equal(changed.status(), 200, "new messages invalidate the previous conditional response")
  assert.notEqual(changed.headers().etag, etag)
  assert.match(await changed.text(), /KRO cache invalidation/)
  const repeated = await context.request.get("/first_run", { maxRedirects: 0 })
  assert.equal(repeated.status(), 302)
  assert.equal(new URL(repeated.headers().location, server.origin).pathname, "/")
  const repeatedPost = await context.request.post("/first_run", {
    maxRedirects: 0,
    headers: { "Sec-Fetch-Site": "same-origin" },
    multipart: { "user[name]": "Unexpected", "user[email_address]": "unexpected@example.test", "user[password]": password },
  })
  assert.equal(repeatedPost.status(), 302)
  const { page: signin, context: signedInContext } = await newPage(t, server)
  await signin.goto("/session/new")
  await profile(signin, "legacy")
  await signin.locator("#email_address").fill("ada@example.test")
  await signin.locator("#password").fill(password)
  await signin.locator('button[name="log_in"]').click()
  await signin.locator("#message-area").waitFor()

  // Reuse the real Basecoat document as a fixture now that /first_run correctly rejects repeats.
  // A frame targeting a legacy page must promote to a full visit, not mix both style profiles.
  await signedInContext.route("**/__kro_basecoat", (route) => route.fulfill({ contentType: "text/html", body: setupHTML }))
  await signin.goto("/__kro_basecoat")
  await signin.evaluate(() => {
    window.kroDocumentMarker = "basecoat-frame"
    const frame = document.createElement("turbo-frame")
    frame.id = "kro-profile-frame"
    frame.src = "/users/me/profile"
    document.body.append(frame)
  })
  await signin.waitForURL("**/users/me/profile")
  await signin.waitForLoadState("load")
  await profile(signin, "legacy")
  assert.equal(await signin.evaluate(() => window.kroDocumentMarker), undefined, "a cross-profile frame creates a new document")
})

test("avatar preview and upload survive signup", { timeout: 60_000 }, async (t) => {
  const server = await freshServer(t)
  const { page } = await newPage(t, server)
  await page.goto("/first_run")
  await page.locator("#user_name").waitFor()
  await page.locator("#user_avatar").setInputFiles(path.join(repo, "reference/test/fixtures/files/moon.jpg"))
  const preview = page.locator('[data-upload-preview-target="image"]')
  await preview.waitFor()
  await page.waitForFunction(() => document.querySelector('[data-upload-preview-target="image"]')?.src.startsWith("blob:"))
  await fillSignup(page, "avatar@example.test")
  await submitSignup(page)
  await page.goto("/users/me/profile")
  await page.getByRole("button", { name: "Remove avatar", exact: true }).waitFor({ state: "attached" })
  const src = await page.locator('[data-upload-preview-target="image"]').first().getAttribute("src")
  const avatar = await page.request.get(src)
  assert.equal(avatar.status(), 200, "stored avatar can be read after signup")
  assert.match(avatar.headers()["content-type"], /^image\//)
})

test("required validation and native submission work without JavaScript", { timeout: 60_000 }, async (t) => {
  const server = await freshServer(t)
  const { page } = await newPage(t, server, { javaScriptEnabled: false })
  await page.goto("/first_run")
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  assert.equal(new URL(page.url()).pathname, "/first_run")
  assert.equal(await page.locator("#user_name").evaluate((input) => input.validity.valueMissing), true)
  await page.locator("#user_name").fill("Ada Browser")
  await page.locator("#user_email_address").fill("not-an-email")
  await page.locator("#user_password").fill(password)
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  assert.equal(await page.locator("#user_email_address").evaluate((input) => input.validity.typeMismatch), true)
  assert.equal(await page.locator("#user_password").getAttribute("maxlength"), "72")
  assert.equal(await page.locator("[data-password-toggle]").isVisible(), false, "password toggle is a JavaScript enhancement")
  await page.locator("#user_email_address").fill("native@example.test")
  await page.locator("#user_password").fill("1234567")
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  assert.equal(new URL(page.url()).pathname, "/first_run")
  assert.equal(await page.locator("#user_password").evaluate((input) => input.validity.tooShort), true, "native validation requires eight characters")
  await page.locator("#user_password").fill("12345678")
  await submitSignup(page)
})

test("bypassing browser validation still rejects short passwords and preserves non-secret fields", { timeout: 60_000 }, async (t) => {
  const server = await freshServer(t)
  const { page, context } = await newPage(t, server)
  await page.goto("/first_run")
  await fillSignup(page, "minimum@example.test")
  await page.locator("#user_password").fill("1234567")
  await page.locator('form[action="/first_run"]').evaluate((form) => { form.noValidate = true })
  const rejected = page.waitForResponse((response) => response.request().method() === "POST" && new URL(response.url()).pathname === "/first_run")
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  assert.equal((await rejected).status(), 422)
  await page.getByRole("alert").filter({ hasText: "Password must be at least 8 characters." }).waitFor()
  assert.equal(await page.locator("#user_name").inputValue(), "Ada Browser")
  assert.equal(await page.locator("#user_email_address").inputValue(), "minimum@example.test")
  assert.equal(await page.locator("#user_password").inputValue(), "")
  assert.equal(await page.locator("#user_password").getAttribute("aria-invalid"), "true")
  await page.waitForFunction(() => document.activeElement?.id === "user_password")
  assert.equal((await context.request.get("/first_run", { maxRedirects: 0 })).status(), 200, "failed setup has not created an account")
  await page.screenshot({ path: path.join(artifacts, "first-run-password-error.png"), fullPage: true })
  await page.locator("#user_password").fill("12345678")
  await submitSignup(page)
})

test("appearance follows the system without a selector and honors saved preferences before paint", { timeout: 60_000 }, async (t) => {
  const server = await freshServer(t)
  const { page, context } = await newPage(t, server, { colorScheme: "dark" })
  await context.addInitScript(() => {
    new MutationObserver((_records, observer) => {
      if (document.body) {
        window.kroDarkWhenBodyAppeared = document.documentElement.classList.contains("dark")
        observer.disconnect()
      }
    }).observe(document, { childList: true, subtree: true })
  })
  await page.goto("/first_run")
  assert.equal(await page.locator("[data-appearance-select]").count(), 0)
  await expectDark(page, true)
  assert.equal(await page.evaluate(() => window.kroDarkWhenBodyAppeared), true)
  await page.emulateMedia({ colorScheme: "light" })
  await expectDark(page, false)
  // Existing preferences from earlier versions remain valid even without a selector on setup.
  await page.evaluate(() => localStorage.setItem("matchbox:appearance", "dark"))
  await page.reload()
  await expectDark(page, true)
  assert.equal(await page.evaluate(() => window.kroDarkWhenBodyAppeared), true)
  await page.evaluate(() => localStorage.setItem("matchbox:appearance", "light"))
  await page.emulateMedia({ colorScheme: "dark" })
  await page.reload()
  await expectDark(page, false)
  assert.equal(await page.evaluate(() => window.kroDarkWhenBodyAppeared), false)
  await page.evaluate(() => localStorage.removeItem("matchbox:appearance"))
  await page.reload()
  await expectDark(page, true)
})

test("first-run visual contract on desktop and a 320-pixel phone in both modes", { timeout: 60_000 }, async (t) => {
  const server = await freshServer(t)
  const { page } = await newPage(t, server)
  for (const [name, width, height] of [["desktop", 1280, 900], ["phone", 320, 760]]) {
    await page.setViewportSize({ width, height })
    for (const scheme of ["light", "dark"]) {
      await page.emulateMedia({ colorScheme: scheme })
      await page.goto("/first_run")
      await expectDark(page, scheme === "dark")
      await page.getByRole("heading", { name: "Set up Matchbox", exact: true }).waitFor()
      await profile(page, "basecoat")
      const geometry = await page.evaluate(() => {
        const form = document.querySelector('form[action="/first_run"]')
        const box = form.getBoundingClientRect()
        return { left: box.left, right: box.right, width: box.width, scrollWidth: document.documentElement.scrollWidth, viewport: innerWidth }
      })
      assert.ok(geometry.scrollWidth <= width, `${name} has no horizontal overflow`)
      assert.ok(geometry.left >= 12 && geometry.right <= width - 12, `${name} retains side gutters`)
      if (name === "desktop") {
        assert.ok(geometry.width <= 600, "form remains a bounded desktop card")
        assert.ok(Math.abs(geometry.left - (width - geometry.right)) <= 2, "desktop card is centered")
      }
      for (const id of ["user_name", "user_email_address", "user_password"]) {
        const input = page.locator(`#${id}`)
        const label = page.locator(`label[for="${id}"]`)
        assert.equal(await label.isVisible(), true, `${id} has a persistent visible label`)
        const labelBox = await label.boundingBox()
        const inputBox = await input.boundingBox()
        assert.ok(inputBox.height >= 34 && inputBox.width >= 180, `${id} has usable dimensions`)
        assert.ok(labelBox.y + labelBox.height <= inputBox.y + 2, `${id} label is above the field`)
      }
      assert.equal(await page.locator('[data-controller="popup"]').count(), 0, "setup has no language selector")
      assert.equal(await page.locator("select").count(), 0, "setup has no appearance selector")
      await page.locator("#user_password").fill("check-visibility")
      const toggle = page.locator("[data-password-toggle]")
      await toggle.click()
      assert.equal(await page.locator("#user_password").getAttribute("type"), "text")
      assert.equal(await toggle.getAttribute("aria-label"), "Hide password")
      assert.equal(await toggle.getAttribute("aria-pressed"), "true")
      await toggle.click()
      assert.equal(await page.locator("#user_password").getAttribute("type"), "password")
      assert.equal(await page.locator("#user_password").inputValue(), "check-visibility")
      await page.locator("#user_password").clear()
      const colors = await page.locator("#user_email_address").evaluate((input) => {
        const canvas = document.createElement("canvas")
        canvas.width = canvas.height = 1
        const ctx = canvas.getContext("2d")
        const rgb = (color) => {
          ctx.clearRect(0, 0, 1, 1)
          ctx.fillStyle = color
          ctx.fillRect(0, 0, 1, 1)
          return [...ctx.getImageData(0, 0, 1, 1).data]
        }
        const foreground = getComputedStyle(input).color
        const foregroundRgb = rgb(foreground)
        const ancestors = []
        for (let element = input; element; element = element.parentElement) ancestors.unshift(element)
        // Dark inputs have translucent white backgrounds; composite their ancestors as the browser
        // does instead of treating that white tint as the final surface color.
        ctx.fillStyle = "white"
        ctx.fillRect(0, 0, 1, 1)
        for (const element of ancestors) {
          ctx.fillStyle = getComputedStyle(element).backgroundColor
          ctx.fillRect(0, 0, 1, 1)
        }
        return { foreground: foregroundRgb, background: [...ctx.getImageData(0, 0, 1, 1).data] }
      })
      const luminance = (rgb) => rgb.slice(0, 3).map((byte) => byte / 255).map((x) => x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4).reduce((sum, x, i) => sum + x * [0.2126, 0.7152, 0.0722][i], 0)
      const bg = luminance(colors.background)
      const fg = luminance(colors.foreground)
      assert.ok((Math.max(bg, fg) + 0.05) / (Math.min(bg, fg) + 0.05) >= 4.5, "input text has readable contrast")
      assert.ok(scheme === "dark" ? bg < 0.1 : bg > 0.8, `${scheme} mode changes the actual form surface`)
      await page.locator("#user_name").focus()
      await page.keyboard.press("Tab")
      const focus = await page.locator(":focus").evaluate((element) => ({ visible: element.matches(":focus-visible"), outline: getComputedStyle(element).outlineStyle, shadow: getComputedStyle(element).boxShadow }))
      assert.equal(focus.visible, true)
      assert.ok(focus.outline !== "none" || focus.shadow !== "none", "keyboard focus has a visible treatment")
      await page.locator(":focus").blur()
      await page.mouse.move(0, 0)
      await page.screenshot({ path: path.join(artifacts, `first-run-${name}-${scheme}.png`), fullPage: true, animations: "disabled", caret: "hide" })
    }
  }
})

test("Turbo restores Basecoat controls without duplicating password handlers", { timeout: 60_000 }, async (t) => {
  const server = await freshServer(t)
  const { page, context } = await newPage(t, server)
  // Setup cannot be revisited after success. Serve a second unmodified setup document at a test-only
  // URL to exercise the same-profile Turbo navigation and its snapshot restoration beforehand.
  const html = await (await context.request.get("/first_run")).text()
  await context.route("**/__kro_next", (route) => route.fulfill({ contentType: "text/html", body: html }))
  await page.goto("/first_run")
  await page.emulateMedia({ colorScheme: "dark" })
  await expectDark(page, true)
  await page.locator("#user_name").fill("Preserved draft")
  await page.evaluate(() => {
    window.kroDocumentMarker = "same-profile"
    window.kroComponentClicks = 0
    window.basecoat.register("krofixture", ".kro-fixture", (element) => {
      if (element.hasAttribute("data-krofixture-initialized")) return
      const clicked = () => { window.kroComponentClicks += 1 }
      element.addEventListener("click", clicked)
      element.setAttribute("data-krofixture-initialized", "")
      element._destroy = () => element.removeEventListener("click", clicked)
    })
    const control = document.createElement("button")
    control.className = "kro-fixture"
    control.textContent = "KRO component"
    document.body.append(control)
    const link = document.createElement("a")
    link.href = "/__kro_next"
    link.textContent = "KRO next page"
    document.body.append(link)
  })
  await page.locator(".kro-fixture[data-krofixture-initialized]").waitFor()
  await page.getByText("KRO next page", { exact: true }).click()
  await page.waitForURL("**/__kro_next")
  await page.waitForFunction(() => document.querySelector("#user_name")?.value === "")
  assert.equal(await page.evaluate(() => window.kroDocumentMarker), "same-profile", "same-profile navigation uses Turbo")
  await page.evaluate(() => {
    const control = document.createElement("button")
    control.className = "kro-fixture"
    control.textContent = "KRO component"
    document.body.append(control)
  })
  await page.locator(".kro-fixture[data-krofixture-initialized]").waitFor()
  await page.goBack()
  await page.waitForURL("**/first_run")
  await page.waitForFunction(() => document.querySelector("#user_name")?.value === "Preserved draft")
  assert.equal(await page.locator("#user_name").inputValue(), "Preserved draft")
  await page.getByText("KRO component", { exact: true }).click()
  assert.equal(await page.evaluate(() => window.kroComponentClicks), 1, "cached component regains exactly one click handler")
  await profile(page, "basecoat")
  const toggle = page.locator("[data-password-toggle]")
  await toggle.click()
  assert.equal(await page.locator("#user_password").getAttribute("type"), "text", "restored toggle runs exactly once")
  await page.goForward()
  await page.waitForURL("**/__kro_next")
  await page.waitForFunction(() => document.querySelector("#user_name")?.value === "")
  assert.equal(await page.locator("#user_password").getAttribute("type"), "password")
  await toggle.click()
  assert.equal(await page.locator("#user_password").getAttribute("type"), "text", "forward restoration also binds one password handler")
  await page.getByText("KRO component", { exact: true }).click()
  assert.equal(await page.evaluate(() => window.kroComponentClicks), 2, "forward restoration also binds one handler")
})
