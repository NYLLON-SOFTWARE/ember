// Deliberate differences between the Rust app and the reference (README "Known differences") that
// show in the network layer, masked on both sides so the rest of the layer still has to match.
//
// - Cookies are only sent when they change: Rails re-sends `_campfire_session`, `session_token`
//   and `last_room` on nearly every response, and the Rust app deletes an emptied session cookie.
// - The web app manifest is JSON-escaped rather than HTML-escaped, so its body differs.
// - The stock account logo uses Ember artwork. Only the known stock image bytes are normalized;
//   uploaded workspace logos, HTTP status, and response headers remain compared.

import { createHash } from "node:crypto"
import fs from "node:fs"
import path from "node:path"
import { REPO_DIR } from "./config.ts"
import { normalizeResponse } from "./normalize.ts"

const SESSION_COOKIES = /^ {2}set-cookie: (_campfire_session|session_token|last_room)\b.*\n/gm
const STOCK_LOGO_BODIES = {
  small: stockLogoBodies("app-icon-192.png"),
  large: stockLogoBodies("app-icon.png"),
}

function stockLogoBodies(filename: string): Set<string> {
  return new Set(["reference/app/assets/images/logos", "crates/assets/overrides/logos"].flatMap((dir) => {
    const file = path.join(REPO_DIR, dir, filename)
    if (!fs.existsSync(file)) return []
    const normalized = normalizeResponse(fs.readFileSync(file), "image/png")
    return [createHash("sha256").update(normalized).digest("hex").slice(0, 16)]
  }))
}

export function maskDeliberateNetworkDifferences(text: string): string {
  return text
    .replace(SESSION_COOKIES, "")
    .replace(/^( {2}headers:.*) set-cookie\b/gm, "$1")
    .replace(/^(GET \/webmanifest\.json\b.*\n(?: {2}.*\n)*? {2}body: )sha256:[0-9a-f]+/gm, "$1«manifest»")
    .replace(/^(GET \/account\/logo(?:\?[^ \n]*)? → 200\n(?: {2}.*\n)*? {2}body: )sha256:([0-9a-f]+)/gm,
      (response, head, hash) => {
        const size = /[?&]size=small(?:&| )/.test(head) ? "small" : "large"
        return STOCK_LOGO_BODIES[size].has(hash) ? `${head}«stock-account-logo»` : response
      })
}
