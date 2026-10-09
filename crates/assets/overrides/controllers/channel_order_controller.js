import { Controller } from "@hotwired/stimulus"
import { put } from "@rails/request.js"
import { ignoringBriefDisconnects } from "helpers/dom_helpers"

// Admin settings stage the shared room order; sidebar drags only reorder personal favorites.
// Touch scrolling stays native until a deliberate hold starts a sidebar drag.
export default class extends Controller {
  static targets = [ "list", "room", "reset", "status", "save" ]
  static values = { order: Array, staged: Boolean, favorites: Array, url: String }

  #candidate
  #dragging = false
  #saving = false
  #hold
  #scrollFrame
  #suppressClickUntil = 0
  #events
  #refreshPending = false
  #refreshing = false
  #savedOrder = []

  connect() {
    this.#events = new AbortController()
    const options = { signal: this.#events.signal }
    window.addEventListener("matchbox:room-order-changed", () => {
      this.#refreshPending = true
      this.#refresh()
    }, options)
    window.addEventListener("matchbox:favorites-changed", event => {
      if (this.stagedValue) return
      this.#cancel()
      this.favoritesValue = event.detail.favorites
      this.sort()
    }, options)
    this.element.addEventListener("pointerdown", this.#pointerDown, options)
    window.addEventListener("pointermove", this.#pointerMove, options)
    window.addEventListener("pointerup", this.#pointerUp, options)
    window.addEventListener("pointercancel", this.#cancel, options)
    this.element.addEventListener("touchstart", this.#touchStart, { ...options, passive: true })
    window.addEventListener("touchmove", this.#touchMove, { ...options, passive: false })
    window.addEventListener("touchend", this.#touchEnd, options)
    window.addEventListener("touchcancel", this.#cancel, options)
    this.element.addEventListener("keydown", this.#keyDown, options)
    this.element.addEventListener("dragstart", event => event.preventDefault(), options)
    this.element.addEventListener("contextmenu", event => {
      if (this.#dragging) event.preventDefault()
    }, options)
    this.element.addEventListener("click", event => {
      if (Date.now() < this.#suppressClickUntil && event.target.closest('[data-channel-order-target="room"]')) {
        event.preventDefault()
        event.stopImmediatePropagation()
      }
    }, { ...options, capture: true })
    document.addEventListener("turbo:before-cache", this.#cancel, options)
    document.addEventListener("turbo:before-frame-render", this.#preserveInteraction, options)
    if (!this.#dragging) this.sort()
    this.#savedOrder = this.#ids()
    this.#updateButtons()
  }

  disconnect() {
    this.#events?.abort()
    ignoringBriefDisconnects(this.element, this.#cancel)
  }

  async #refresh() {
    if (!this.#refreshPending || this.#candidate || this.#saving || this.#refreshing || !this.element.isConnected) return
    const frame = this.element.closest("turbo-frame")
    if (!frame) return
    this.#refreshing = true
    await frame.loaded
    this.#refreshing = false
    if (this.#candidate || this.#saving || !this.element.isConnected) return
    this.#refreshPending = false
    const focusId = this.element.contains(document.activeElement) ? document.activeElement.id : null
    const scroller = this.element.closest(".mb-conversations")
    const scrollTop = scroller?.scrollTop || 0
    frame.reload()
    await frame.loaded
    const nextScroller = frame.querySelector(".mb-conversations")
    if (nextScroller) nextScroller.scrollTop = scrollTop
    if (focusId) document.getElementById(focusId)?.focus({ preventScroll: true })
  }

  #preserveInteraction = event => {
    if ((!this.#candidate && !this.#saving) || event.target !== this.element.closest("turbo-frame")) return
    const incoming = event.detail.newFrame.querySelector(`#${this.element.id}`)
    if (!incoming) return
    // Cable reconnects refresh the sidebar. Let Turbo's Bardo move this live section so a
    // background refresh cannot discard a hold, drag, or a save already in flight.
    const sections = [this.element, incoming].map(section => [section, section.hasAttribute("data-turbo-permanent")])
    for (const [section] of sections) section.setAttribute("data-turbo-permanent", "")
    const render = event.detail.render
    event.detail.render = (...args) => {
      try {
        return render(...args)
      } finally {
        for (const [section, permanent] of sections) section.toggleAttribute("data-turbo-permanent", permanent)
      }
    }
  }

  roomTargetConnected() {
    // Broadcasts insert and replace channel links independently of this controller.
    queueMicrotask(() => {
      if (this.element.isConnected && !this.#dragging) this.sort()
    })
  }

  sort() {
    const positions = new Map(this.orderValue.map((id, index) => [ String(id), index ]))
    const favorites = new Map(this.favoritesValue.map((id, index) => [String(id), index]))
    const rows = this.roomTargets.slice().sort((a, b) => {
      const favoriteRank = this.stagedValue ? 0 : (favorites.get(a.dataset.roomId) ?? Infinity) - (favorites.get(b.dataset.roomId) ?? Infinity)
      const rank = (positions.get(a.dataset.roomId) ?? Infinity) - (positions.get(b.dataset.roomId) ?? Infinity)
      return (Number.isNaN(favoriteRank) ? 0 : favoriteRank) || (Number.isNaN(rank) ? 0 : rank) || a.dataset.sortedListName.toLowerCase().localeCompare(b.dataset.sortedListName.toLowerCase())
    })
    // Avoid a mutation loop when Stimulus reconnects a moved target.
    rows.forEach((row, index) => {
      if (!this.stagedValue) {
        let marker = row.querySelector(".mb-room-favorite-marker")
        if (!marker) {
          marker = document.createElement("span")
          marker.className = "mb-room-favorite-marker"
          marker.textContent = "★"
          marker.setAttribute("role", "img")
          marker.setAttribute("aria-label", "Favorite")
          row.append(marker)
        }
        const hidden = !favorites.has(row.dataset.roomId)
        if (marker.hidden !== hidden) marker.hidden = hidden
        const handle = row.querySelector(".mb-room-drag-handle")
        if (handle) handle.hidden = hidden
        row.classList.toggle("mb-room-reorderable", !hidden)
        if (hidden) row.removeAttribute("aria-keyshortcuts")
        else row.setAttribute("aria-keyshortcuts", "Alt+ArrowUp Alt+ArrowDown")
      }
      if (this.listTarget.children[index] !== row) this.listTarget.insertBefore(row, this.listTarget.children[index] || null)
    })
    this.#updateButtons()
  }

  reset() {
    if (this.#saving || !this.stagedValue) return
    this.orderValue = this.#savedOrder.slice()
    this.sort()
    this.statusTarget.hidden = true
  }

  save() {
    if (!this.#saving && this.stagedValue) this.#save(this.#ids())
  }

  #updateButtons() {
    if (!this.stagedValue) return
    const unchanged = JSON.stringify(this.#ids()) === JSON.stringify(this.#savedOrder)
    this.saveTarget.disabled = this.#saving || unchanged
    this.resetTarget.disabled = this.#saving || unchanged
  }

  #movableRows() {
    return this.roomTargets.filter(row => !row.hidden && (this.stagedValue || this.favoritesValue.includes(Number(row.dataset.roomId))))
  }

  #stageOrSave(order) {
    if (!this.stagedValue) return this.#save(order)
    this.orderValue = order
    this.sort()
    this.#announce("Unsaved room order. Save to apply it to everyone.")
  }

  #room(event) {
    const row = event.target.closest('[data-channel-order-target="room"]')
    return row && this.#movableRows().includes(row) ? row : null
  }

  #start(event, point, kind) {
    const row = this.#room(event)
    if (!row || this.#saving || this.#movableRows().length < 2) return
    if (this.stagedValue && !event.target.closest("[data-room-order-handle]")) return
    this.#cancel()
    this.#candidate = { row, x: point.clientX, y: point.clientY, lastY: point.clientY, kind }
    const begin = () => {
      if (!this.#candidate?.row.isConnected) return this.#cancel()
      this.#dragging = true
      this.#candidate.row.classList.add("mb-channel-dragging")
      this.listTarget.classList.add("mb-channel-order-active")
      this.#announce(`Moving ${row.dataset.sortedListName}. Release to ${this.stagedValue ? "place" : "save"}, or press Escape to cancel.`)
      this.#scroll()
    }
    if (this.stagedValue) begin()
    else this.#hold = setTimeout(begin, 400)
  }

  #move(point) {
    const candidate = this.#candidate
    if (!candidate) return
    if (!this.#dragging) {
      if (Math.hypot(point.clientX - candidate.x, point.clientY - candidate.y) > 8) this.#cancel()
      return
    }
    candidate.lastY = point.clientY
    const otherRows = this.#movableRows().filter(row => row !== candidate.row)
    const after = otherRows.find(row => point.clientY < row.getBoundingClientRect().top + row.getBoundingClientRect().height / 2)
    if (after) this.listTarget.insertBefore(candidate.row, after)
    else {
      const firstFixed = this.roomTargets.find(row => !this.#movableRows().includes(row))
      this.listTarget.insertBefore(candidate.row, firstFixed || null)
    }
  }

  #scroll = () => {
    if (!this.#dragging || !this.#candidate) return
    const conversations = this.stagedValue ? this.listTarget : this.element.closest(".mb-conversations")
    if (conversations) {
      const box = conversations.getBoundingClientRect()
      const y = this.#candidate.lastY
      const distance = y < box.top + 64 ? -8 : y > box.bottom - 64 ? 8 : 0
      const previous = conversations.scrollTop
      conversations.scrollTop += distance
      // A held pointer can stay still while the rows underneath it scroll past.
      if (conversations.scrollTop !== previous) this.#move({ clientX: this.#candidate.x, clientY: y })
    }
    this.#scrollFrame = requestAnimationFrame(this.#scroll)
  }

  #finish() {
    if (!this.#candidate) return
    const dragged = this.#dragging
    const row = this.#candidate.row
    const order = this.#ids()
    this.#cleanUp()
    if (dragged) {
      this.#suppressClickUntil = Date.now() + 500
      row.focus({ preventScroll: true })
      this.#stageOrSave(order)
    } else this.#refresh()
  }

  #cleanUp() {
    clearTimeout(this.#hold)
    cancelAnimationFrame(this.#scrollFrame)
    this.#candidate?.row.classList.remove("mb-channel-dragging")
    if (this.hasListTarget) this.listTarget.classList.remove("mb-channel-order-active")
    this.#candidate = null
    this.#dragging = false
  }

  #cancel = () => {
    const dragged = this.#dragging
    this.#cleanUp()
    if (dragged) {
      this.#suppressClickUntil = Date.now() + 500
      this.sort()
      this.#announce("Room move canceled.")
    }
    this.#refresh()
  }

  #pointerDown = event => {
    if (event.pointerType !== "touch" && event.button === 0) this.#start(event, event, "pointer")
  }

  #pointerMove = event => {
    if (this.#candidate?.kind === "pointer") {
      if (this.#dragging) event.preventDefault()
      this.#move(event)
    }
  }

  #pointerUp = () => {
    if (this.#candidate?.kind === "pointer") this.#finish()
  }

  #touchStart = event => {
    if (event.touches.length === 1) this.#start(event, event.touches[0], "touch")
    else this.#cancel()
  }

  #touchMove = event => {
    if (this.#candidate?.kind === "touch") {
      if (this.#dragging && event.cancelable) event.preventDefault()
      this.#move(event.touches[0])
    }
  }

  #touchEnd = () => {
    if (this.#candidate?.kind === "touch") this.#finish()
  }

  #keyDown = event => {
    if (event.key === "Escape" && this.#dragging) {
      event.preventDefault()
      return this.#cancel()
    }
    const row = this.#room(event)
    if (!row || !event.altKey || ![ "ArrowUp", "ArrowDown" ].includes(event.key)) return
    event.preventDefault()
    if (this.#saving) return
    const rows = this.#movableRows()
    const index = rows.indexOf(row)
    const next = rows[index + (event.key === "ArrowUp" ? -1 : 1)]
    if (!next) return
    if (event.key === "ArrowUp") this.listTarget.insertBefore(row, next)
    else this.listTarget.insertBefore(next, row)
    row.focus({ preventScroll: true })
    this.#stageOrSave(this.#ids())
  }

  #ids() {
    const movable = new Set(this.#movableRows())
    return Array.from(this.listTarget.children).filter(row => movable.has(row)).map(row => Number(row.dataset.roomId))
  }

  async #save(order) {
    const previous = this.favoritesValue.slice()
    this.#saving = true
    if (!this.stagedValue) this.favoritesValue = order
    this.sort()
    this.#announce("Saving room order…")
    try {
      const response = await put(this.urlValue, { body: { room_ids: order }, responseKind: "json" })
      if (!response.ok) throw new Error("Room order was not saved")
      this.#savedOrder = order.slice()
      window.Turbo?.cache.clear()
      this.#announce(this.stagedValue ? "Room order saved for everyone." : "Starred room order saved.")
    } catch {
      if (!this.stagedValue) this.favoritesValue = previous
      this.sort()
      this.#announce("Couldn’t save room order. Please try again.", true)
    } finally {
      this.#saving = false
      this.#updateButtons()
      this.#refresh()
    }
  }

  #announce(message, error = false) {
    this.statusTarget.hidden = false
    this.statusTarget.classList.toggle("for-screen-reader", !error && !this.stagedValue)
    this.statusTarget.classList.toggle("mb-order-error", error)
    this.statusTarget.textContent = message
  }
}
