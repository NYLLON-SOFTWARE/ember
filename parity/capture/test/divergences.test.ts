import { test } from "node:test"
import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import fs from "node:fs"
import path from "node:path"
import { REPO_DIR } from "../config.ts"
import { maskDeliberateNetworkDifferences } from "../divergences.ts"
import { normalizeResponse } from "../normalize.ts"

const rails = `GET /session/new (navigation) → 200
  headers: cache-control content-type etag set-cookie vary
  set-cookie: _campfire_session; httponly; path; samesite
  set-cookie: session_token; httponly; path; samesite
  body: sha256:1e79006cec2b3d32
GET /webmanifest.json → 200
  headers: cache-control content-type set-cookie
  content-type: application/json; charset=utf-8
  body: sha256:2c2aaf5a651296e1
`

const rust = `GET /session/new (navigation) → 200
  headers: cache-control content-type etag vary
  body: sha256:1e79006cec2b3d32
GET /webmanifest.json → 200
  headers: cache-control content-type
  content-type: application/json; charset=utf-8
  body: sha256:0b1c2d3e4f5a6b7c
`

test("masks session cookie writes and the manifest's body", () => {
  assert.equal(maskDeliberateNetworkDifferences(rails), maskDeliberateNetworkDifferences(rust))
})

test("other cookies and bodies still differ", () => {
  const other = rust.replace("body: sha256:1e79006cec2b3d32", "body: sha256:ffffffffffffffff")
  assert.notEqual(maskDeliberateNetworkDifferences(rails), maskDeliberateNetworkDifferences(other))
  const cookie = rust.replace("  headers: cache-control content-type etag vary\n", "  headers: cache-control content-type etag vary\n  set-cookie: flash; path\n")
  assert.notEqual(maskDeliberateNetworkDifferences(rails), maskDeliberateNetworkDifferences(cookie))
})

test("only known stock account-logo bytes are masked, preserving custom logos and HTTP contracts", () => {
  const head = `GET /account/logo?size=small&v=1 → 200
  headers: cache-control content-type etag
  content-type: image/png
`
  const response = (file: string) => {
    const bytes = fs.readFileSync(path.join(REPO_DIR, file))
    const hash = createHash("sha256").update(normalizeResponse(bytes, "image/png")).digest("hex").slice(0, 16)
    return `${head}  body: sha256:${hash}\n`
  }
  const original = response("reference/app/assets/images/logos/app-icon-192.png")
  const ember = response("crates/assets/overrides/logos/app-icon-192.png")
  const masked = maskDeliberateNetworkDifferences(original)
  assert.equal(masked, maskDeliberateNetworkDifferences(ember))
  assert.match(masked, /body: «stock-account-logo»/)

  const custom = `${head}  body: sha256:ffffffffffffffff\n`
  assert.equal(maskDeliberateNetworkDifferences(custom), custom)
  assert.notEqual(masked, maskDeliberateNetworkDifferences(original.replace("→ 200", "→ 404")))
  assert.notEqual(masked, maskDeliberateNetworkDifferences(original.replace("image/png", "image/jpeg")))
  const avatar = original.replace("/account/logo", "/users/1/avatar")
  assert.equal(maskDeliberateNetworkDifferences(avatar), avatar)
  const wrongSize = response("crates/assets/overrides/logos/app-icon.png")
  assert.equal(maskDeliberateNetworkDifferences(wrongSize), wrongSize, "small and large icons must not be interchangeable")
  const large = wrongSize.replace("?size=small&v=1", "?v=1")
  assert.match(maskDeliberateNetworkDifferences(large), /body: «stock-account-logo»/)
})
