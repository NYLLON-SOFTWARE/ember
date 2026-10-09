import { Controller } from "@hotwired/stimulus"

let activeController

export default class extends Controller {
  static targets = [ "trigger", "menu", "more", "tray" ]

  connect() {
    this.moreTarget.hidden = false
    this.trayTarget.hidden = true
    this.element.classList.add("mb-message-actions--positioned")
    this.hoverEvents = new AbortController()
    const options = { signal: this.hoverEvents.signal }
    const message = this.element.closest(".message")
    message?.addEventListener("pointerenter", (event) => this.hover(event), options)
    message?.addEventListener("pointerleave", () => this.leave(), options)
    // Delegate so the inline control still works after Turbo replaces the boosts frame.
    message?.addEventListener("click", (event) => {
      const trigger = event.target.closest(".message__boost-inline .boost__action")
      if (!trigger || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
      event.preventDefault()
      this.boostTrigger = trigger
      this.hoverOpened = false
      clearTimeout(this.closeTimer)
      this.element.open = true
      this.trayTarget.hidden = false
      this.moreTarget.setAttribute("aria-expanded", "true")
      this.toggle()
      this.trayTarget.querySelector("button, a")?.focus({ preventScroll: true })
    }, options)
    // Keep the overlapping bar and its expanded reaction tray part of the same hover region.
    this.menuTarget.addEventListener("pointerenter", () => clearTimeout(this.closeTimer), options)
    this.menuTarget.addEventListener("pointerleave", () => this.leave(), options)
    this.triggerTarget.addEventListener("click", (event) => {
      if (this.element.open && this.hoverOpened) {
        event.preventDefault()
        this.hoverOpened = false
        clearTimeout(this.closeTimer)
      }
    }, options)
    this.toggle()
  }

  disconnect() {
    this.hoverEvents?.abort()
    clearTimeout(this.closeTimer)
    this.stopObserving()
  }

  hover(event) {
    if (event.pointerType === "touch" || !matchMedia("(hover: hover) and (pointer: fine)").matches) return
    clearTimeout(this.closeTimer)
    if (this.element.open) return
    this.hoverOpened = true
    this.element.open = true
    this.toggle()
  }

  leave() {
    clearTimeout(this.closeTimer)
    this.closeTimer = setTimeout(() => {
      if (this.hoverOpened && !this.element.matches(":focus-within") && !this.element.closest(".message")?.matches(":hover")) this.close()
    }, 180)
  }

  toggle() {
    this.triggerTarget.setAttribute("aria-expanded", String(this.element.open))
    this.stopObserving()
    if (!this.element.open) {
      this.hoverOpened = false
      this.boostTrigger = undefined
      this.trayTarget.hidden = true
      this.moreTarget.setAttribute("aria-expanded", "false")
      return
    }

    if (activeController && activeController !== this) activeController.close()
    activeController = this
    // Observe only the open bar, rather than installing global handlers for every message.
    this.events = new AbortController()
    const options = { signal: this.events.signal }
    document.addEventListener("click", (event) => {
      if (!this.element.contains(event.target) && !this.boostTrigger?.contains(event.target)) this.close()
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
    const trigger = this.boostTrigger || this.triggerTarget
    this.boostTrigger = undefined
    clearTimeout(this.closeTimer)
    this.hoverOpened = false
    this.element.open = false
    this.triggerTarget.setAttribute("aria-expanded", "false")
    this.trayTarget.hidden = true
    this.moreTarget.setAttribute("aria-expanded", "false")
    this.stopObserving()
    if (event?.type === "submit") trigger.focus({ preventScroll: true })
  }

  more() {
    this.trayTarget.hidden = !this.trayTarget.hidden
    this.moreTarget.setAttribute("aria-expanded", String(!this.trayTarget.hidden))
    this.position()
  }

  keydown(event) {
    if (event.key !== "Escape") return
    event.preventDefault()
    if (this.boostTrigger) {
      const trigger = this.boostTrigger
      this.close()
      trigger.focus({ preventScroll: true })
    } else if (!this.trayTarget.hidden) {
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
    const overlapping = matchMedia("(hover: hover) and (pointer: fine)").matches
    this.element.classList.toggle("mb-message-actions--overlapping", overlapping)
    const trigger = this.triggerTarget.getBoundingClientRect()
    const row = this.element.closest(".message")?.querySelector(".message__body")?.getBoundingClientRect() || trigger
    if (row.bottom < bounds.top || row.top > bounds.bottom) {
      this.close()
      return
    }
    this.menuTarget.style.maxWidth = `${Math.max(0, bounds.right - bounds.left)}px`
    this.trayTarget.style.maxHeight = ""
    let menu = this.menuTarget.getBoundingClientRect()
    let y
    let barTop
    let barHeight
    if (overlapping) {
      barHeight = this.menuTarget.querySelector(".mb-message-action-bar").getBoundingClientRect().height
      // Overlap the row by more than half the bar, preserving a continuous pointer path.
      barTop = Math.max(bounds.top, Math.min(row.top - 17, bounds.bottom - barHeight))
      const below = bounds.bottom - barTop - barHeight
      const above = barTop - bounds.top
      const trayAbove = !this.trayTarget.hidden && menu.height - barHeight > below && above > below
      this.element.classList.toggle("mb-message-actions--tray-above", trayAbove)
      if (!this.trayTarget.hidden) {
        this.trayTarget.style.maxHeight = `${Math.max(0, (trayAbove ? above : below) - 7)}px`
        menu = this.menuTarget.getBoundingClientRect()
      }
      // Expanding the tray must not pull the bar away from its row or out from under the pointer.
      y = trayAbove ? barTop - (menu.height - barHeight) : barTop
    } else {
      this.element.classList.remove("mb-message-actions--tray-above")
      const below = trigger.bottom + 6
      const preferredY = below + menu.height <= bounds.bottom ? below : trigger.top - menu.height - 6
      y = Math.max(bounds.top, Math.min(preferredY, bounds.bottom - menu.height))
    }
    const right = overlapping ? bounds.right - 8 : trigger.right
    const x = Math.max(bounds.left, Math.min(right - menu.width, bounds.right - menu.width))
    this.menuTarget.style.left = `${x}px`
    this.menuTarget.style.top = `${y}px`
    if (overlapping) {
      // The native disclosure stays keyboard-accessible as the last visible button in the bar.
      this.element.style.setProperty("--mb-action-trigger-left", `${x + menu.width - 5 - trigger.width}px`)
      this.element.style.setProperty("--mb-action-trigger-top", `${barTop + (barHeight - trigger.height) / 2}px`)
    }
  }

  stopObserving() {
    if (activeController === this) activeController = undefined
    this.events?.abort()
    this.observer?.disconnect()
  }
}
