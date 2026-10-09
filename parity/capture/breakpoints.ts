// The breakpoint sweep: find every width condition in reference/app/assets/stylesheets and capture
// 1px either side of it. Ember's breakpoints are in `ch`, which media queries resolve against
// the engine's *initial* font, so each engine is probed with matchMedia rather than computed.
import fs from "node:fs"
import path from "node:path"
import type { Browser } from "playwright"
import { STYLESHEETS_DIR } from "./config.ts"

export interface WidthCondition {
  feature: "min-width" | "max-width"
  length: string // e.g. "100ch"
  sources: string[] // file:line
}

export function scanWidthConditions(dir = STYLESHEETS_DIR): WidthCondition[] {
  const found = new Map<string, WidthCondition>()
  const add = (feature: WidthCondition["feature"], length: string, source: string) => {
    const key = `${feature}:${length}`
    if (!found.has(key)) found.set(key, { feature, length, sources: [] })
    found.get(key)!.sources.push(source)
  }
  for (const file of fs.readdirSync(dir).filter((f) => f.endsWith(".css")).sort()) {
    const lines = fs.readFileSync(path.join(dir, file), "utf8").split("\n")
    lines.forEach((line, i) => {
      if (!/@media|@container/.test(line)) return
      const source = `${file}:${i + 1}`
      for (const m of line.matchAll(/\((min|max)-width\s*:\s*([\d.]+[a-z%]*)\s*\)/g)) {
        add(`${m[1]}-width` as WidthCondition["feature"], m[2], source)
      }
      // Range syntax: (width > 40em), (width <= 100ch), (40em < width)
      for (const m of line.matchAll(/\(\s*width\s*(>=?|<=?)\s*([\d.]+[a-z%]*)\s*\)/g)) {
        add(m[1].startsWith(">") ? "min-width" : "max-width", m[2], source)
      }
      for (const m of line.matchAll(/\(\s*([\d.]+[a-z%]*)\s*(<=?|>=?)\s*width\s*\)/g)) {
        add(m[2].startsWith("<") ? "min-width" : "max-width", m[1], source)
      }
    })
  }
  return [...found.values()]
}

// For each condition, the widths where it flips: the last non-matching and first matching integer
// width. Returns the union, sorted.
export async function resolveBreakpointWidths(browser: Browser, conditions: WidthCondition[]): Promise<number[]> {
  const context = await browser.newContext({ viewport: { width: 1000, height: 900 }, deviceScaleFactor: 1 })
  const page = await context.newPage()
  await page.setContent(`<!DOCTYPE html><html lang="en"><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body></body></html>`)
  const matches = async (width: number, query: string) => {
    await page.setViewportSize({ width, height: 900 })
    return page.evaluate((q) => matchMedia(q).matches, query)
  }
  const widths = new Set<number>()
  for (const condition of conditions) {
    const query = `(${condition.feature}: ${condition.length})`
    // min-width matches for all widths >= the breakpoint; max-width for all widths <= it.
    const wantsLarge = condition.feature === "min-width"
    let lo = 1, hi = 4000
    if ((await matches(lo, query)) === (await matches(hi, query))) continue // never flips in range
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1
      if ((await matches(mid, query)) === wantsLarge) hi = mid
      else lo = mid
    }
    widths.add(lo).add(hi)
  }
  await context.close()
  return [...widths].sort((a, b) => a - b)
}
