import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = [ "trigger", "menu", "more", "tray" ]

  connect() {
    this.moreTarget.hidden = false
    this.trayTarget.hidden = true
    this.element.classList.add("mb-message-actions--positioned")
    this.toggle()
  }

  disconnect() {
    this.stopObserving()
  }

  toggle() {
    this.triggerTarget.setAttribute("aria-expanded", String(this.element.open))
    this.stopObserving()
    if (!this.element.open) {
      this.trayTarget.hidden = true
      this.moreTarget.setAttribute("aria-expanded", "false")
      return
    }

    // Observe only the open bar, rather than installing global handlers for every message.
    this.events = new AbortController()
    const options = { signal: this.events.signal }
    document.addEventListener("click", (event) => {
      if (!this.element.contains(event.target)) this.close()
    }, options)
    document.addEventListener("keydown", (event) => this.keydown(event), options)
    document.addEventListener("focusin", (event) => {
      if (!this.element.contains(event.target)) this.close()
    }, options)
    document.addEventListener("scroll", () => this.position(), { ...options, capture: true, passive: true })
    document.addEventListener("turbo:before-cache", () => this.close(), options)
    window.addEventListener("resize", () => this.position(), options)
    window.visualViewport?.addEventListener("resize", () => this.position(), options)
    this.observer = new ResizeObserver(() => this.position())
    this.observer.observe(this.menuTarget)
    this.position()
  }

  close(event) {
    this.element.open = false
    this.triggerTarget.setAttribute("aria-expanded", "false")
    this.trayTarget.hidden = true
    this.moreTarget.setAttribute("aria-expanded", "false")
    this.stopObserving()
    if (event?.type === "submit") this.triggerTarget.focus({ preventScroll: true })
  }

  more() {
    this.trayTarget.hidden = !this.trayTarget.hidden
    this.moreTarget.setAttribute("aria-expanded", String(!this.trayTarget.hidden))
    this.position()
  }

  keydown(event) {
    if (event.key !== "Escape") return
    event.preventDefault()
    if (!this.trayTarget.hidden) {
      this.trayTarget.hidden = true
      this.moreTarget.setAttribute("aria-expanded", "false")
      this.moreTarget.focus({ preventScroll: true })
      this.position()
    } else {
      this.close()
      this.triggerTarget.focus({ preventScroll: true })
    }
  }

  position() {
    if (!this.element.open) return
    const viewport = window.visualViewport
    const top = viewport?.offsetTop || 0
    const left = viewport?.offsetLeft || 0
    const width = viewport?.width || window.innerWidth
    const height = viewport?.height || window.innerHeight
    const timeline = this.element.closest(".messages, #search-results")?.getBoundingClientRect()
    const bounds = {
      left: Math.max(left, timeline?.left ?? left) + 8,
      right: Math.min(left + width, timeline?.right ?? left + width) - 8,
      top: Math.max(top, timeline?.top ?? top) + 8,
      bottom: Math.min(top + height, timeline?.bottom ?? top + height) - 8,
    }
    const trigger = this.triggerTarget.getBoundingClientRect()
    if (trigger.bottom < bounds.top || trigger.top > bounds.bottom) {
      this.close()
      return
    }
    this.menuTarget.style.maxWidth = `${Math.max(0, bounds.right - bounds.left)}px`
    const menu = this.menuTarget.getBoundingClientRect()
    const x = Math.max(bounds.left, Math.min(trigger.right - menu.width, bounds.right - menu.width))
    const preferredY = trigger.bottom + 6 + menu.height <= bounds.bottom
      ? trigger.bottom + 6
      : trigger.top - menu.height - 6
    const y = Math.max(bounds.top, Math.min(preferredY, bounds.bottom - menu.height))
    this.menuTarget.style.left = `${x}px`
    this.menuTarget.style.top = `${y}px`
  }

  stopObserving() {
    this.events?.abort()
    this.observer?.disconnect()
  }
}
