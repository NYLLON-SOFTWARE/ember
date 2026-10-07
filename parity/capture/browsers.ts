// Browser processes and context options for each support-matrix cell.
import { chromium, firefox, webkit, devices } from "playwright"
import type { Browser, BrowserContextOptions, BrowserType } from "playwright"
import { LOCALE, TIMEZONE } from "./config.ts"
import type { Engine } from "./config.ts"
import type { Cell } from "./inventory.ts"

const TYPES: Record<Engine, BrowserType> = { chromium, firefox, webkit }

// One capture at a time per browser process (reused sequentially, replaced after a while so long
// runs don't accumulate state). Sharing a process between concurrent captures was not neutral:
// WebKit and Firefox give window focus to whichever page was created last, so autofocused fields
// drew their focus ring or not depending on what else was running, and Chromium shares its
// decoded-image cache, so the lightbox's 3840x2160 JPEG came out of it at a different scale.
const CONTEXTS_PER_BROWSER: Record<Engine, number> = { chromium: 1, firefox: 1, webkit: 1 }
const RECYCLE_AFTER = 200

interface Slot {
  browser: Promise<Browser>
  active: number
  served: number
}

export class BrowserPool {
  private slots: Record<string, Slot[]> = {}
  private versions: Partial<Record<Engine, string>> = {}

  async acquire(engine: Engine): Promise<{ browser: Browser; release: () => Promise<void> }> {
    const slots = (this.slots[engine] ??= [])
    let slot = slots.find((s) => s.active < CONTEXTS_PER_BROWSER[engine] && s.served < RECYCLE_AFTER)
    if (!slot) {
      slot = { browser: launch(engine), active: 0, served: 0 }
      slots.push(slot)
    }
    slot.active++
    slot.served++
    const chosen = slot
    const browser = await chosen.browser
    this.versions[engine] ??= browser.version()
    return {
      browser,
      release: async () => {
        chosen.active--
        if (chosen.served >= RECYCLE_AFTER && chosen.active === 0) {
          slots.splice(slots.indexOf(chosen), 1)
          await browser.close().catch(() => {})
        }
      },
    }
  }

  version(engine: Engine): string | undefined {
    return this.versions[engine]
  }

  async close() {
    const all = Object.values(this.slots).flat()
    this.slots = {}
    await Promise.all(all.map(async (s) => (await s.browser).close().catch(() => {})))
  }
}

// Chromium rasterizes in tiles on several threads, and antialiased edges crossing tile or
// partial-raster boundaries came out one gray level apart between identical runs. These make its
// software raster single-pass and deterministic. No GPU: it renders with SwiftShader/Skia.
// Subpixel glyph positioning is off too: with it, the same text at the same layout position came
// out a device pixel apart between identical runs (the mention autocomplete's item text on the
// phone viewport, about 1 in 8 captures) with the layout identical to 1/64px and no matter how
// the layer was repainted or recreated; without it, 40 of 40 captures were identical. Glyph
// origins then snap to whole device pixels, the same way for both servers.
const CHROMIUM_ARGS = [
  "--disable-font-subpixel-positioning",
  // Skia caches rasterized masks across draws, and a border drawn from the cache came out a gray
  // level apart from one rastered fresh (the sidebar toggle's circle on the phone viewport after
  // auth/join/completed, in about half the captures): the pixels depended on what had been
  // painted before. Without the cache, 8 of 8 were identical.
  "--skia-resource-cache-limit-mb=0",
  "--disable-gpu",
  "--disable-gpu-rasterization",
  "--disable-partial-raster",
  "--num-raster-threads=1",
  "--disable-threaded-animation",
  "--disable-threaded-scrolling",
  "--disable-checker-imaging",
  "--run-all-compositor-stages-before-draw",
  "--disable-lcd-text",
  "--force-color-profile=srgb",
  "--disable-skia-runtime-opts",
  "--disable-features=PaintHolding",
]

// Firefox otherwise bypasses the proxy for localhost, and then treats the proxied localhost as
// an insecure origin; the shared origin (proxy.ts) is localhost so it stays a secure context.
const FIREFOX_PREFS = {
  "network.proxy.allow_hijacking_localhost": true,
  "network.proxy.testing_localhost_is_secure_when_hijacked": true,
}

function launch(engine: Engine): Promise<Browser> {
  return TYPES[engine].launch({
    headless: true,
    args: engine === "chromium" ? CHROMIUM_ARGS : [],
    firefoxUserPrefs: engine === "firefox" ? FIREFOX_PREFS : undefined,
  })
}

export function contextOptions(cell: Cell, browserVersion: string): BrowserContextOptions {
  const { viewport, engine } = cell
  const options: BrowserContextOptions = {
    viewport: { width: viewport.width, height: viewport.height },
    screen: { width: viewport.width, height: viewport.height },
    deviceScaleFactor: viewport.deviceScaleFactor,
    hasTouch: viewport.touch,
    colorScheme: cell.scheme,
    reducedMotion: "no-preference",
    forcedColors: "none",
    timezoneId: TIMEZONE,
    locale: LOCALE,
    // Matchbox's service worker (reference/app/views/pwa/service_worker.js) handles push and
    // notification clicks only, never fetch, so allowing it hides no requests from the harness.
    serviceWorkers: "allow",
    ignoreHTTPSErrors: true,
  }
  // Firefox has no mobile emulation mode; it still gets touch and a mobile UA.
  if (viewport.mobile && engine !== "firefox") options.isMobile = true
  if (viewport.mobile) options.userAgent = mobileUserAgent(engine, browserVersion)
  return options
}

function mobileUserAgent(engine: Engine, version: string): string {
  const major = version.split(".")[0]
  switch (engine) {
    case "chromium":
      return devices["Pixel 7"].userAgent.replace(/Chrome\/[\d.]+/, `Chrome/${major}.0.0.0`)
    case "webkit":
      return devices["iPhone 15"].userAgent
    case "firefox":
      return `Mozilla/5.0 (Android 14; Mobile; rv:${major}.0) Gecko/${major}.0 Firefox/${major}.0`
  }
}
