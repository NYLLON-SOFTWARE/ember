import { Controller } from "@hotwired/stimulus"

const maximumBytes = 5 * 1024 * 1024

export default class extends Controller {
  static targets = [ "image" ]
  static values = { url: String }

  connect() {
    if (this.imageTarget.getAttribute("src") && !this.imageTarget.hidden) return
    this.imageTarget.removeAttribute("src")
    const abort = this.abort = new AbortController()
    const observer = this.observer = new IntersectionObserver((entries) => {
      if (!abort.signal.aborted && entries.some((entry) => entry.isIntersecting)) {
        observer.disconnect()
        this.load(abort.signal)
      }
    }, { rootMargin: "200px" })
    this.observer.observe(this.element)
  }

  disconnect() {
    this.observer?.disconnect()
    this.abort?.abort()
  }

  async load(signal) {
    try {
      const url = new URL(this.urlValue, window.location.href)
      if (url.origin !== window.location.origin) return
      const response = await fetch(url, { signal, credentials: "same-origin" })
      if (!response.ok || Number(response.headers.get("Content-Length")) > maximumBytes) {
        await response.body?.cancel()
        return
      }
      const reader = response.body.getReader()
      const chunks = []
      let length = 0
      try {
        while (true) {
          const { done, value } = await reader.read()
          if (done) break
          length += value.byteLength
          if (length > maximumBytes) return
          chunks.push(value)
        }
      } finally {
        await reader.cancel()
      }
      const dataURL = await this.asDataURL(new Blob(chunks, { type: "image/svg+xml" }))
      if (signal.aborted) return
      // An image context disables SVG scripts and external resources. A data URL also
      // has an opaque origin if opened separately; do not use a same-origin blob URL,
      // inline SVG, <object>, or <iframe> for uploaded markup.
      const image = this.imageTarget
      image.src = dataURL
      await image.decode()
      if (!signal.aborted) image.hidden = false
    } catch {
      if (!signal.aborted) this.imageTarget.removeAttribute("src")
      // The original file card remains the fallback for invalid or unsupported SVGs.
    }
  }

  asDataURL(blob) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader()
      reader.onload = () => resolve(reader.result)
      reader.onerror = () => reject(reader.error)
      reader.readAsDataURL(blob)
    })
  }
}
