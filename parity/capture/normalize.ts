// Normalized HTML for the DOM layer (plans/rust-conversion.md, "Layer 2"). parse5 is a WHATWG
// spec parser like html5ever, so both servers' bytes land in the same tree the browser builds.
// The output is one node per line with sorted attributes and collapsed whitespace, so a line diff
// reads like a DOM diff. Volatile values become *typed* placeholders: a token for the wrong record
// still differs, and timestamps are decoded relative to the seed clock instead of masked away.
import { createHash } from "node:crypto"
import { parse, parseFragment } from "parse5"
import { maskProductBranding } from "./branding.ts"

export interface NormalizeOptions {
  seedTime?: number // epoch ms: the seed's clock.now, which timestamps are decoded against
  frozenServerClock?: boolean // servers run with --freeze: expiries are exact too
}

const RAW_TEXT = new Set(["pre", "textarea", "listing", "plaintext", "xmp"])
const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"])

export function normalizeDocument(html: string, options: NormalizeOptions = {}): string {
  const lines: string[] = []
  walk(parse(closeReferenceTransferForm(html)), 0, lines, options, false)
  return lines.join("\n") + "\n"
}

// The frozen Rails transfer template leaves its auto-submit form open, wrapping the footer.
// Current implementations explicitly close it (README, Known differences). Normalize only
// this one empty PUT form; native GET tests independently require its explicit closing tag.
export function closeReferenceTransferForm(html: string): string {
  const forms = [...html.matchAll(/<form\b[^>]*>/g)].filter((form) =>
    /\bdata-controller="auto-submit"/.test(form[0]) &&
    /\baction="(?:https?:\/\/[^"/]+)?\/session\/transfers\/[^"<>]+"/.test(form[0]))
  if (forms.length !== 1) return html
  const form = forms[0]
  const start = form.index! + form[0].length
  const fields = /^(?:\s*<input\b[^>]*>)+/.exec(html.slice(start))
  if (!fields || !/<input\b(?=[^>]*\bname="_method")(?=[^>]*\bvalue="put")[^>]*>/.test(fields[0])) return html
  const end = start + fields[0].length
  if (!/^\s*<footer\b[^>]*\bid="footer"/.test(html.slice(end))) return html
  return html.slice(0, end) + "</form>" + html.slice(end)
}

export function normalizeFragment(html: string, options: NormalizeOptions = {}): string {
  const lines: string[] = []
  walk(parseFragment(html), 0, lines, options, false)
  return lines.join("\n") + "\n"
}

function walk(node: any, depth: number, out: string[], options: NormalizeOptions, raw: boolean) {
  const indent = "  ".repeat(depth)
  switch (node.nodeName) {
    case "#document":
    case "#document-fragment":
      for (const child of node.childNodes) walk(child, depth, out, options, raw)
      return
    case "#documentType":
      out.push(`${indent}<!DOCTYPE ${node.name}>`)
      return
    case "#comment": {
      const text = collapse(node.data)
      if (text) out.push(`${indent}<!-- ${text} -->`)
      return
    }
    case "#text": {
      if (raw) {
        if (node.value) out.push(`${indent}${JSON.stringify(maskText(node.value, options))}`)
      } else {
        const text = collapse(node.value)
        if (text) out.push(`${indent}${maskText(text, options)}`)
      }
      return
    }
  }
  const tag = node.tagName as string
  if (isForgeryToken(tag, node.attrs)) return
  const attrs = node.attrs
    .map((a: any) => relativeCopyLink(node.attrs, a))
    .sort((a: any, b: any) => a.name.localeCompare(b.name))
    .map((a: any) => formatAttr(tag, a.name, maskAttr(tag, a.name, a.value, node.attrs, options)))
  out.push(`${indent}<${tag}${attrs.join("")}>`)
  const children = tag === "template" ? node.content.childNodes : node.childNodes
  const childRaw = raw || RAW_TEXT.has(tag)
  if (tag === "script" || tag === "style") {
    const text = children.map((c: any) => c.value ?? "").join("")
    const body = tag === "script" && /importmap|json/.test(attrValue(node, "type") ?? "") ? normalizeJson(text) : collapse(text)
    if (body) out.push(`${indent}  ${maskText(body, options)}`)
  } else {
    for (const child of children) walk(child, depth + 1, out, options, childRaw)
  }
  if (!VOID.has(tag)) out.push(`${indent}</${tag}>`)
}

function formatAttr(_tag: string, name: string, value: string): string {
  return value === "" ? ` ${name}` : ` ${name}=${JSON.stringify(value)}`
}

function attrValue(node: any, name: string): string | undefined {
  return node.attrs.find((a: any) => a.name === name)?.value
}

function collapse(text: string): string {
  return text.replace(/[\t\n\f\r ]+/g, " ").trim()
}

function normalizeJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text))
  } catch {
    return collapse(text)
  }
}

// --- Masking -----------------------------------------------------------------------------------

// The CSRF meta tags and token fields Rails renders. The Rust app checks Sec-Fetch-Site instead and
// renders none (README, Known differences), so they're left out of both sides.
function isForgeryToken(tag: string, attrs: any[]): boolean {
  const name = attrs.find((a: any) => a.name === "name")?.value
  return (tag === "input" && name === "authenticity_token") || (tag === "meta" && (name === "csrf-token" || name === "csrf-param"))
}

// A message's "Copy link" button: Rails puts the absolute URL, built from the request's host, in
// the content value; the Rust app caches that markup for every request, so it carries the path in
// a url value that its copy-to-clipboard controller makes absolute (README, Known differences).
function relativeCopyLink(attrs: any[], attr: any): any {
  const title = attrs.find((a: any) => a.name === "title")?.value
  const url = /^https?:\/\/[^\/]+(\/.*)$/.exec(attr.value)
  if (title !== "Copy link" || attr.name !== "data-copy-to-clipboard-content-value" || !url) return attr
  return { name: "data-copy-to-clipboard-url-value", value: url[1] }
}

function maskAttr(tag: string, name: string, value: string, attrs: any[], options: NormalizeOptions): string {
  const attr = (n: string) => attrs.find((a: any) => a.name === n)?.value
  if (name === "nonce" || (tag === "meta" && attr("name") === "csp-nonce" && name === "content")) return "«nonce»"
  const epochMs = relativeEpoch(name, value, options)
  if (epochMs) return epochMs
  return maskText(value, options)
}

export function maskText(text: string, options: NormalizeOptions): string {
  return maskProductBranding(text)
    .replace(QR_CODE, (_, encoded) => `/qr_code/«qr:${maskText(decodeBase64(encoded)?.toString("utf8") ?? encoded, options)}»`)
    .replace(ISO_TIME, (iso) => relativeIso(iso, options))
    .replace(SIGNED_TOKEN, (token) => describeSignedToken(token, options))
    .replace(ENCRYPTED_MESSAGE, "«encrypted»")
    .replace(BLOB_URL, "blob:«object-url»")
    .replace(ASSET_DIGEST, (_, name, _digest, ext) => `/assets/${name}-«digest»${ext}`)
}

// URL.createObjectURL: a random UUID per call (upload previews).
const BLOB_URL = /blob:[a-z]+:\/\/[^\/\s"]+\/[0-9a-f-]{36}/g

// QrCodeController takes the URL to encode as base64 in the path.
const QR_CODE = /\/qr_code\/([A-Za-z0-9_=-]{8,})/g

// Propshaft: /assets/name-<8+ hex>.ext (also .digested.js files keep their name).
const ASSET_DIGEST = /\/assets\/([^\s"'()?#]+?)-([0-9a-f]{7,64})(\.[A-Za-z0-9.]+)/g

// ActiveSupport::MessageVerifier output: base64(data)--hexdigest, possibly URL-escaped.
const SIGNED_TOKEN = /(?<![A-Za-z0-9+%_-])((?:[A-Za-z0-9+\/_-]|%2[BbFf]|%3[Dd])+={0,2}(?:%3[Dd]){0,2})--([0-9a-f]{40,128})(?![0-9a-f])/g

// ActiveSupport::MessageEncryptor output: base64--base64--base64.
const ENCRYPTED_MESSAGE = /(?<![A-Za-z0-9+\/])[A-Za-z0-9+\/]{16,}={0,2}--[A-Za-z0-9+\/]{12,}={0,2}--[A-Za-z0-9+\/]{16,}={0,2}(?![A-Za-z0-9+\/])/g

const ISO_TIME = /\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})\b/g

// Standard base64 includes "/", so a match can swallow the URL path in front of the token
// (/rails/active_storage/blobs/redirect/<token>). Keep the longest suffix that decodes.
function describeSignedToken(token: string, options: NormalizeOptions): string {
  const [encoded] = token.split("--")
  const starts = [0, ...[...encoded.matchAll(/\//g)].map((m) => m.index! + 1)]
  for (const start of starts) {
    const description = describeSignedMessage(encoded.slice(start), options)
    if (description) return encoded.slice(0, start) + description
  }
  const last = starts[starts.length - 1]
  return encoded.slice(0, last) + "«signed»"
}

function describeSignedMessage(encoded: string, options: NormalizeOptions): string | undefined {
  const data = decodeBase64(safeDecodeURIComponent(encoded))
  if (!data) return
  const text = data.toString("latin1")
  const json = parseJson(text)
  const envelope = json?._rails
  if (envelope) {
    let payload = envelope.data
    if (payload === undefined && typeof envelope.message === "string") {
      const inner = decodeBase64(envelope.message)?.toString("utf8")
      payload = inner !== undefined ? parseJson(inner) ?? inner : envelope.message
    }
    const purpose = envelope.pur ?? "default"
    const expires = envelope.exp ? `:${describeExpiry(envelope.exp, options)}` : ""
    const gid = typeof payload === "string" && describeGid(payload)
    if (gid) return `«sgid:${purpose === "default" ? "" : purpose + ":"}${gid}${expires}»`
    return `«signed_id:${purpose}:${typeof payload === "string" ? payload : JSON.stringify(payload)}${expires}»`
  }
  // Marshal-era messages (\x04\x08) and bare ones: pull out a GlobalID if there is one.
  const gid = describeGid(text)
  if (gid) return `«sgid:${gid}»`
  // Turbo signed stream names: a JSON string of ":"-joined GlobalID params (base64 gid://…).
  if (typeof json === "string") return `«stream:${json.split(":").map(describeGidParam).join(":")}»`
  if (json !== undefined) return `«signed:${JSON.stringify(json)}»`
  if (text.startsWith("\x04\x08")) return "«signed:marshal»"
}

// The servers' clocks tick from the seed instant (a frozen libfaketime clock spins Puma), so an
// expiry is "time of rendering + lifetime", and the time of rendering isn't observable: the Date
// header comes from Thruster, outside faketime. The expiry is kept as a typed marker; lifetimes
// are Layer 3's advancing-clock scenarios. With a frozen server clock (--freeze) it is decoded.
function describeExpiry(exp: string, options: NormalizeOptions): string {
  return options.frozenServerClock ? `exp${relativeIso(exp, options)}` : "expires"
}

function describeGidParam(part: string): string {
  const decoded = decodeBase64(part)?.toString("utf8")
  return (decoded && describeGid(decoded)) ?? part
}

function describeGid(text: string): string | undefined {
  const m = /gid:\/\/[^\/\s]+\/([A-Za-z:]+)\/([^\s?"\x00-\x1f]+)/.exec(text)
  return m ? `${m[1]}#${decodeURIComponent(m[2])}` : undefined
}

function relativeIso(iso: string, options: NormalizeOptions): string {
  const t = Date.parse(iso)
  if (Number.isNaN(t) || options.seedTime === undefined) return iso
  return `«t${formatDelta(t - options.seedTime)}»`
}

// data-message-timestamp and friends are epoch milliseconds (config/initializers/time_formats.rb).
function relativeEpoch(name: string, value: string, options: NormalizeOptions): string | undefined {
  if (options.seedTime === undefined || !/^\d{10}(\d{3})?$/.test(value)) return
  const ms = value.length === 13 ? Number(value) : Number(value) * 1000
  const plausible = Math.abs(ms - options.seedTime) < 20 * 365 * 86_400_000
  const timeish = value.length === 13 || /(_at|-at|time|date|timestamp|sort|number|expires)/i.test(name)
  return plausible && timeish ? `«epoch${value.length === 13 ? "ms" : "s"}${formatDelta(ms - options.seedTime)}»` : undefined
}

function formatDelta(ms: number): string {
  const sign = ms < 0 ? "-" : "+"
  const abs = Math.abs(ms)
  return `${sign}${(abs / 1000).toFixed(abs % 1000 === 0 ? 0 : 3)}s`
}

function decodeBase64(text: string): Buffer | undefined {
  if (!/^[A-Za-z0-9+\/_-]+={0,2}$/.test(text)) return
  const buffer = Buffer.from(text.replace(/-/g, "+").replace(/_/g, "/"), "base64")
  return buffer.length ? buffer : undefined
}

function safeDecodeURIComponent(text: string): string {
  try {
    return decodeURIComponent(text)
  } catch {
    return text
  }
}

function parseJson(text: string): any {
  try {
    return JSON.parse(text)
  } catch {
    return undefined
  }
}

// Fragment responses: turbo-stream and HTML fragments as trees, JSON with sorted keys, anything
// else as masked text.
export function normalizeResponse(body: Buffer, contentType: string, options: NormalizeOptions = {}): string {
  const text = body.toString("utf8")
  if (/json/.test(contentType)) {
    const json = parseJson(text)
    if (json !== undefined) return JSON.stringify(maskJson(json, options), null, 2) + "\n"
  }
  if (/html|xml|svg|turbo-stream/.test(contentType)) {
    return /^\s*(<!DOCTYPE|<html)/i.test(text) ? normalizeDocument(text, options) : normalizeFragment(text, options)
  }
  if (/^(text|application\/(javascript|manifest))/.test(contentType)) return maskText(text, options)
  return `«${body.length} bytes sha256:${createHash("sha256").update(body).digest("hex")}»\n`
}

function maskJson(value: any, options: NormalizeOptions, key = ""): any {
  if (Array.isArray(value)) return value.map((v) => maskJson(v, options, key))
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((k) => [k, maskJson(value[k], options, k)]))
  }
  if (typeof value === "string") return maskText(value, options)
  if (typeof value === "number" && Number.isInteger(value)) {
    const epoch = relativeEpoch(key, String(value), options)
    if (epoch) return epoch
  }
  return value
}
