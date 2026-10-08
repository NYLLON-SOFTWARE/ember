import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { Allowlist } from "../allowlist.ts"
import { artifactBase } from "../capture.ts"
import { compareJob } from "../compare.ts"
import { ALLOWLIST_FILE, SCREENS_FILE, VIEWPORTS } from "../config.ts"
import { loadInventory } from "../inventory.ts"
import type { Job } from "../inventory.ts"

const job: Job = {
  state: { id: "auth/sign_in", path: "/session/new", seed: "default", steps: [] },
  cell: { engine: "chromium", viewport: VIEWPORTS.desktop, scheme: "light" },
}
const presentation = new Allowlist([{ state: "**", layers: ["server"], reason: "Owned presentation", owner: "views" }])

for (const kind of ["page", "fragment"]) {
  test(`${kind} HTTP status mismatches cannot be hidden by a presentation exception`, (t) => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "matchbox-parity-status-"))
    t.after(() => fs.rmSync(dir, { recursive: true, force: true }))
    for (const [target, status] of [["rails", 200], ["matchbox", 500]] as const) {
      const base = artifactBase(dir, target, job)
      fs.mkdirSync(path.dirname(base), { recursive: true })
      fs.writeFileSync(base + ".json", JSON.stringify({ kind, status }))
    }
    const result = compareJob(job, dir, "rails", "matchbox", presentation)
    assert.equal(result.status, "fail")
    assert.equal(result.layers.length, 1)
    assert.equal(result.layers[0].equal, false)
    assert.equal(result.layers[0].allowed, undefined)
  })
}

test("a reviewed HTML exception still applies when HTTP status matches", (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "matchbox-parity-html-"))
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }))
  for (const target of ["rails", "matchbox"]) {
    const base = artifactBase(dir, target, job)
    fs.mkdirSync(path.dirname(base), { recursive: true })
    fs.writeFileSync(base + ".json", JSON.stringify({ kind: "fragment", status: 200 }))
    fs.writeFileSync(base + ".server.norm.html", `<h1>${target}</h1>`)
  }
  assert.equal(compareJob(job, dir, "rails", "matchbox", presentation).status, "allowed")
})

test("workspace presentation entries keep network, Cable, and protocol fragments strict", () => {
  const allowlist = Allowlist.load(ALLOWLIST_FILE)
  const states = loadInventory(SCREENS_FILE)
  for (const state of states) {
    assert.equal(allowlist.match(state.id, "chromium-desktop-light", "network"), undefined, state.id)
    assert.equal(allowlist.match(state.id, "chromium-desktop-light", "cable"), undefined, state.id)
    const messagePresentation = ["messages/index/fragment", "messages/show/fragment", "rooms/refresh/fragment"]
    if (state.kind === "fragment" && !state.id.startsWith("pwa/manifest") && !messagePresentation.includes(state.id)) {
      assert.equal(allowlist.match(state.id, "chromium-desktop-light", "server"), undefined, state.id)
    }
  }
  assert.equal(allowlist.match("future/new_page", "chromium-desktop-light", "server"), undefined)
})
