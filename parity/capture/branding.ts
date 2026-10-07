import type { Page } from "playwright"

// Keep upstream captures as evidence. Normalize only the deliberate product-name difference.
export function maskProductBranding(text: string): string {
  return text.replaceAll("Campfire", "Matchbox")
    .replace(/\/assets\/(?:campfire|matchbox)-icon-[0-9a-f]{7,64}\.png/g, "/assets/«product-icon».png")
}

// Equalize application copy before rasterization rather than exempting entire screenshots.
// Message contents and editable user data remain untouched.
export async function normalizeBrandingForScreenshot(page: Page): Promise<void> {
  await page.evaluate(() => {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT)
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.parentElement?.closest("script, style, textarea, [contenteditable], #messages, .message")) continue
      if (node.textContent?.includes("Campfire")) node.textContent = node.textContent.replaceAll("Campfire", "Matchbox")
    }
  })
}
