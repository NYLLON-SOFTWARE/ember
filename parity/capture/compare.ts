// Compares two captures of the same jobs: pixels exactly, then normalized server HTML, live DOM
// and the accessibility tree as line diffs.
import fs from "node:fs"
import path from "node:path"
import type { Allowlist, AllowEntry, Layer } from "./allowlist.ts"
import { comparePngs, diffText } from "./diff.ts"
import type { PixelResult, TextDiff } from "./diff.ts"
import { artifactBase } from "./capture.ts"
import type { CellMeta } from "./capture.ts"
import { cellId } from "./inventory.ts"
import { maskDeliberateNetworkDifferences } from "./divergences.ts"
import { maskOverriddenAssets } from "./overrides.ts"
import type { Job } from "./inventory.ts"

export type Status = "pass" | "fail" | "allowed" | "error"

export interface LayerResult {
  layer: Layer
  equal: boolean
  allowed?: AllowEntry
  pixels?: PixelResult
  text?: TextDiff
}

export interface CellComparison {
  state: string
  cell: string
  status: Status
  error?: string
  layers: LayerResult[]
  expected: { base: string; meta?: CellMeta }
  actual: { base: string; meta?: CellMeta }
  diffImage?: string
  // Pixel flake policy (run.ts): the earlier captures of a cell whose server output matched but
  // whose pixels didn't, and whether a later capture then matched.
  flaky?: boolean
  attempts?: Attempt[]
  masks?: string[] // the state's masks, as the report lists them
}

export interface Attempt {
  attempt: number
  status: Status
  differentPixels?: number
  sizeMismatch?: string
  images?: { expected: string; actual: string; diff?: string }
}

export const SERVER_OUTPUT_LAYERS: Layer[] = ["server", "live", "aria", "network", "cable"]

// A failure that re-capturing may clear: only the pixels differ (not allowed), and every
// server-output layer is identical. Anything else the servers sent differently is never retried.
export function pixelOnlyFailure(result: CellComparison): boolean {
  if (result.status !== "fail") return false
  const failing = result.layers.filter((l) => !l.equal && !l.allowed)
  return failing.length > 0 && failing.every((l) => l.layer === "pixels") && result.layers.filter((l) => SERVER_OUTPUT_LAYERS.includes(l.layer)).every((l) => l.equal)
}

export function describeMasks(masks: Job["state"]["masks"]): string[] | undefined {
  if (!masks) return undefined
  const out = [
    ...Object.entries(masks.values ?? {}).map(([name, m]) => `«${name}»: ${m.selector}${m.attribute ? ` [${m.attribute}]` : ""}${m.match ? ` ~ /${m.match}/` : ""}`),
    ...(masks.pixels ?? []).map((selector) => `pixels: ${selector}`),
  ]
  return out.length ? out : undefined
}

const TEXT_LAYERS: [Layer, string][] = [
  ["server", ".server.norm.html"], ["live", ".live.norm.html"], ["aria", ".aria.yml"], ["network", ".network.txt"], ["cable", ".cable.txt"],
]

export function compareJob(job: Job, runDir: string, expectedName: string, actualName: string, allowlist: Allowlist): CellComparison {
  const cell = cellId(job.cell)
  const expectedBase = artifactBase(runDir, expectedName, job)
  const actualBase = artifactBase(runDir, actualName, job)
  const result: CellComparison = {
    state: job.state.id,
    cell,
    status: "pass",
    layers: [],
    expected: { base: expectedBase, meta: readMeta(expectedBase) },
    actual: { base: actualBase, meta: readMeta(actualBase) },
    masks: describeMasks(job.state.masks),
  }
  const errors = [
    !result.expected.meta && `${expectedName}: not captured`,
    result.expected.meta?.error && `${expectedName}: ${result.expected.meta.error}`,
    !result.actual.meta && `${actualName}: not captured`,
    result.actual.meta?.error && `${actualName}: ${result.actual.meta.error}`,
  ].filter(Boolean)
  if (errors.length) {
    result.status = "error"
    result.error = errors.join("\n")
    return result
  }
  // A presentation exception may allow different HTML, never a different HTTP outcome.
  // Return before finish() so a broad server-layer entry cannot hide a status regression.
  if (result.expected.meta!.status !== result.actual.meta!.status) {
    result.status = "fail"
    result.layers.push({
      layer: "server",
      equal: false,
      text: diffText(`HTTP ${result.expected.meta!.status}`, `HTTP ${result.actual.meta!.status}`),
    })
    return result
  }
  if (result.expected.meta!.kind === "fragment") {
    // A response, not a rendering: status, content type and normalized body are one text layer.
    const text = diffText(readText(expectedBase + ".server.norm.html"), readText(actualBase + ".server.norm.html"))
    result.layers.push({ layer: "server", equal: text.equal, text })
    return finish(result, allowlist)
  }
  const diffImage = path.join(runDir, "diff", actualName, job.state.id, `${cell}.png`)
  fs.rmSync(diffImage, { force: true })
  fs.mkdirSync(path.dirname(diffImage), { recursive: true })
  const pixels = comparePngs(expectedBase + ".png", actualBase + ".png", diffImage)
  if (fs.existsSync(diffImage)) result.diffImage = diffImage
  result.layers.push({ layer: "pixels", equal: pixels.equal, pixels })

  for (const [layer, ext] of TEXT_LAYERS) {
    const mask = (text: string) => {
      const masked = maskOverriddenAssets(text)
      return layer === "network" ? maskDeliberateNetworkDifferences(masked) : masked
    }
    const expected = mask(readText(expectedBase + ext))
    const actual = mask(readText(actualBase + ext))
    const text = diffText(expected, actual)
    result.layers.push({ layer, equal: text.equal, text })
  }

  return finish(result, allowlist)
}

function finish(result: CellComparison, allowlist: Allowlist): CellComparison {
  for (const layer of result.layers) {
    if (!layer.equal) layer.allowed = allowlist.match(result.state, result.cell, layer.layer)
  }
  const failing = result.layers.filter((l) => !l.equal)
  result.status = !failing.length ? "pass" : failing.every((l) => l.allowed) ? "allowed" : "fail"
  return result
}

function readMeta(base: string): CellMeta | undefined {
  try {
    return JSON.parse(fs.readFileSync(base + ".json", "utf8"))
  } catch {
    return undefined
  }
}

function readText(file: string): string {
  try {
    return fs.readFileSync(file, "utf8")
  } catch {
    return "(missing)\n"
  }
}
