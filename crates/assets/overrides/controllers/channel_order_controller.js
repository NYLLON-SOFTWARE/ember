import { Controller } from "@hotwired/stimulus"
import { put } from "@rails/request.js"
import { ignoringBriefDisconnects } from "helpers/dom_helpers"

// Admin drags set the workspace default; members can keep a personal room order. Touch scrolling remains
// native until a deliberate hold starts a drag; ordinary clicks keep opening the channel.
export default class extends Controller {
  static targets = [ "list", "room", "reset", "status" ]
  static values = { order: Array, defaultOrder: Array, customized: Boolean, workspace: Boolean, favorites: Array, url: String }

  #candidate
  #dragging = false
  #saving = false
  #hold
  #scrollFrame
  #suppressClickUntil = 0
  #events
  #refreshPending = false
  #refreshing = false

  connect() {
    this.#events = new AbortController()
    const options = { signal: this.#events.signal }
    window.addEventListener("matchbox:room-order-changed", () => {
      this.#refreshPending = true
      this.#refresh()
    }, options)
    window.addEventListener("matchbox:favorites-changed", event => {
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
    const favorites = new Set(this.favoritesValue.map(String))
    const rows = this.roomTargets.slice().sort((a, b) => {
      const favoriteRank = Number(favorites.has(b.dataset.roomId)) - Number(favorites.has(a.dataset.roomId))
      const rank = (positions.get(a.dataset.roomId) ?? Infinity) - (positions.get(b.dataset.roomId) ?? Infinity)
      return favoriteRank || (Number.isNaN(rank) ? 0 : rank) || a.dataset.sortedListName.toLowerCase().localeCompare(b.dataset.sortedListName.toLowerCase())
    })
    // Avoid a mutation loop when Stimulus reconnects a moved target.
    rows.forEach((row, index) => {
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
      if (this.listTarget.children[index] !== row) this.listTarget.insertBefore(row, this.listTarget.children[index] || null)
    })
    this.resetTarget.hidden = !this.customizedValue
  }

  reset() {
    if (!this.#saving) this.#save([])
  }

  #room(event) {
    const row = event.target.closest('[data-channel-order-target="room"]')
    return row && this.listTarget.contains(row) && !row.hidden ? row : null
  }

  #start(event, point, kind) {
    const row = this.#room(event)
    if (!row || this.#saving || this.roomTargets.filter(row => !row.hidden).length < 2) return
    this.#cancel()
    this.#candidate = { row, x: point.clientX, y: point.clientY, lastY: point.clientY, kind }
    this.#hold = setTimeout(() => {
      if (!this.#candidate?.row.isConnected) return this.#cancel()
      this.#dragging = true
      this.#candidate.row.classList.add("mb-channel-dragging")
      this.listTarget.classList.add("mb-channel-order-active")
      this.#announce(`Moving ${row.dataset.sortedListName}. Release to save, or press Escape to cancel.`)
      this.#scroll()
    }, 400)
  }

  #move(point) {
    const candidate = this.#candidate
    if (!candidate) return
    if (!this.#dragging) {
      if (Math.hypot(point.clientX - candidate.x, point.clientY - candidate.y) > 8) this.#cancel()
      return
    }
    candidate.lastY = point.clientY
    const otherRows = this.roomTargets.filter(row => row !== candidate.row && !row.hidden)
    const after = otherRows.find(row => point.clientY < row.getBoundingClientRect().top + row.getBoundingClientRect().height / 2)
    if (after) this.listTarget.insertBefore(candidate.row, after)
    else this.listTarget.append(candidate.row)
  }

  #scroll = () => {
    if (!this.#dragging || !this.#candidate) return
    const conversations = this.element.closest(".mb-conversations")
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
      this.#save(order)
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
    const rows = this.roomTargets.filter(row => !row.hidden)
    const index = rows.indexOf(row)
    const next = rows[index + (event.key === "ArrowUp" ? -1 : 1)]
    if (!next) return
    if (event.key === "ArrowUp") this.listTarget.insertBefore(row, next)
    else this.listTarget.insertBefore(next, row)
    row.focus({ preventScroll: true })
    this.#save(this.#ids())
  }

  #ids() {
    return Array.from(this.listTarget.children, row => Number(row.dataset.roomId))
  }

  async #save(order) {
    const previous = this.orderValue
    const customized = this.customizedValue
    this.#saving = true
    this.resetTarget.disabled = true
    this.orderValue = order.length || this.workspaceValue ? order : this.defaultOrderValue
    this.customizedValue = order.length > 0
    this.sort()
    this.#announce("Saving room order…")
    try {
      const response = await put(this.urlValue, { body: { room_ids: order }, responseKind: "json" })
      if (!response.ok) throw new Error("Room order was not saved")
      if (this.workspaceValue) this.defaultOrderValue = order
      window.Turbo?.cache.clear()
      this.#announce(this.workspaceValue
        ? (order.length ? "Default room order saved for everyone." : "Default room order reset to alphabetical.")
        : (order.length ? "Room order saved." : "Using the workspace default room order."))
    } catch {
      this.orderValue = previous
      this.customizedValue = customized
      this.sort()
      this.#announce("Couldn’t save room order. Please try again.", true)
    } finally {
      this.#saving = false
      this.resetTarget.disabled = false
      this.#refresh()
    }
  }

  #announce(message, error = false) {
    this.statusTarget.hidden = false
    this.statusTarget.classList.toggle("for-screen-reader", !error)
    this.statusTarget.textContent = message
  }
}
