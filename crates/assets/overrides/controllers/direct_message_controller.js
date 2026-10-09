import { Controller } from "@hotwired/stimulus"

// A cached sidebar may still use the hooks from before the Ember rename.
const newDirectSelector = ":is([data-ember-new-direct], [data-matchbox-new-direct])"

// The shared dialog lives outside the permanent sidebar, so background room updates do not
// replace a search or its selected recipients. The ordinary link remains a fallback.
export default class extends Controller {
  static targets = [ "search", "results", "recipients", "status", "start", "more", "retry" ]
  static values = { url: String }

  #events
  #request
  #timer
  #trigger
  #triggerSelector
  #selected = new Map()
  #people = []
  #next
  #active = -1
  #submitting = false

  connect() {
    this.element.dataset.emberDmReady = "true"
    this.#events = new AbortController()
    const options = { signal: this.#events.signal }
    document.addEventListener("click", event => {
      const trigger = event.target.closest(newDirectSelector)
      if (!trigger || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
      event.preventDefault()
      this.#trigger = trigger
      this.#triggerSelector = `${trigger.closest(".mb-rail") ? ".mb-rail" : "#sidebar"} ${newDirectSelector}`
      window.dispatchEvent(new Event("ember:close-sidebar"))
      this.#reset()
      this.element.showModal()
      this.searchTarget.focus()
      this.#load()
    }, { ...options, capture: true })
    document.addEventListener("turbo:before-cache", () => {
      this.element.close()
      this.#reset()
    }, options)
    this.element.addEventListener("turbo:before-fetch-response", event => {
      // Keep recipients and the retry message when an HTML error would replace the workspace.
      if (!event.detail.fetchResponse.succeeded) event.preventDefault()
    }, options)
  }

  disconnect() {
    delete this.element.dataset.emberDmReady
    this.#events?.abort()
    this.#request?.abort()
    clearTimeout(this.#timer)
  }

  close(event) {
    event?.preventDefault()
    if (this.#submitting) return
    this.element.close()
    this.#reset()
    const trigger = this.#trigger?.isConnected ? this.#trigger : document.querySelector(this.#triggerSelector)
    const target = trigger && !trigger.closest("[inert]") ? trigger : document.querySelector("[data-ember-sidebar-toggle], [data-matchbox-sidebar-toggle]")
    target?.focus({ preventScroll: true })
  }

  backdrop(event) {
    if (event.target !== this.element) return
    const box = this.element.getBoundingClientRect()
    if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) this.close(event)
  }

  #reset() {
    clearTimeout(this.#timer)
    this.#request?.abort()
    this.#selected.clear()
    this.#people = []
    this.#next = undefined
    this.#submitting = false
    this.searchTarget.value = ""
    this.statusTarget.textContent = ""
    this.moreTarget.hidden = true
    this.retryTarget.hidden = true
    this.#renderRecipients()
    this.#renderResults()
    this.statusTarget.textContent = "Search by name or @name."
  }

  #query() { return this.searchTarget.value.trim().replace(/^@+/, "").trim() }

  search() {
    clearTimeout(this.#timer)
    this.#request?.abort()
    this.#people = []
    this.#next = undefined
    this.#renderResults()
    this.moreTarget.hidden = true
    this.retryTarget.hidden = true
    this.statusTarget.textContent = this.#query() ? "Searching…" : "Loading people…"
    this.#timer = setTimeout(() => this.#load(), 180)
  }

  more() { this.#load(this.#next) }

  async #load(next) {
    this.#request?.abort()
    const request = this.#request = new AbortController()
    const url = new URL(next || this.urlValue, location.origin)
    if (!next) url.searchParams.set("query", this.#query())
    this.resultsTarget.setAttribute("aria-busy", "true")
    this.statusTarget.textContent = this.#query() ? "Searching…" : "Loading people…"
    this.moreTarget.hidden = true
    this.retryTarget.hidden = true
    try {
      const response = await fetch(url, { headers: { Accept: "application/json" }, signal: request.signal })
      if (!response.ok) throw new Error("People search failed")
      const people = await response.json()
      if (request.signal.aborted) return
      const currentUser = document.querySelector('meta[name="current-user-id"]')?.content
      const decoder = document.createElement("textarea")
      const existing = new Set(next ? this.#people.map(person => person.id) : [])
      const additions = people.flatMap(person => {
        const id = String(person.value)
        if (id === currentUser || existing.has(id) || !/^\d+$/.test(id)) return []
        existing.add(id)
        decoder.innerHTML = person.name
        return [{ id, name: decoder.value, avatar: person.avatar_url }]
      })
      this.#people = next ? [...this.#people, ...additions] : additions
      const link = response.headers.get("Link")?.match(/<([^>]+)>;\s*rel="next"/)
      const nextURL = link && new URL(link[1], location.origin)
      this.#next = nextURL?.origin === location.origin ? nextURL.href : undefined
      this.moreTarget.hidden = !this.#next
      this.#renderResults()
    } catch (error) {
      if (error.name === "AbortError" || request.signal.aborted) return
      this.statusTarget.textContent = "Couldn’t load people. Please try again."
      this.retryTarget.hidden = false
    } finally {
      if (!request.signal.aborted) this.resultsTarget.removeAttribute("aria-busy")
    }
  }

  #renderResults() {
    this.#active = -1
    this.searchTarget.removeAttribute("aria-activedescendant")
    this.resultsTarget.replaceChildren()
    for (const person of this.#people.filter(person => !this.#selected.has(person.id))) {
      const option = document.createElement("button")
      option.type = "button"
      option.className = "mb-dm-person"
      option.id = `dm-person-${person.id}`
      option.setAttribute("role", "option")
      option.setAttribute("aria-selected", "false")
      option.tabIndex = -1
      const avatar = document.createElement("img")
      avatar.src = person.avatar
      avatar.alt = ""
      avatar.width = avatar.height = 36
      const name = document.createElement("span")
      name.textContent = person.name
      option.append(avatar, name)
      option.addEventListener("click", () => {
        if (this.#submitting) return
        this.#selected.set(person.id, person)
        this.#renderRecipients()
        this.#renderResults()
        this.searchTarget.focus()
      })
      this.resultsTarget.append(option)
    }
    this.statusTarget.textContent = this.resultsTarget.childElementCount ? "Choose one or more people." :
      this.#people.length ? "Everyone shown is selected." : this.#query() ? "No people found. Try another name." :
      "No other people are available to message yet."
  }

  #renderRecipients() {
    this.recipientsTarget.replaceChildren()
    for (const person of this.#selected.values()) {
      const chip = document.createElement("button")
      chip.type = "button"
      chip.className = "mb-dm-chip"
      chip.setAttribute("aria-label", `Remove ${person.name}`)
      chip.textContent = `${person.name} ×`
      chip.addEventListener("click", () => {
        if (this.#submitting) return
        this.#selected.delete(person.id)
        this.#renderRecipients()
        this.#renderResults()
        this.searchTarget.focus()
      })
      const value = document.createElement("input")
      value.type = "hidden"
      value.name = "user_ids[]"
      value.value = person.id
      this.recipientsTarget.append(chip, value)
    }
    this.recipientsTarget.hidden = !this.#selected.size
    this.startTarget.disabled = !this.#selected.size || this.#submitting
  }

  navigate(event) {
    const options = [...this.resultsTarget.children]
    if (["ArrowDown", "ArrowUp"].includes(event.key) && options.length) {
      event.preventDefault()
      const direction = event.key === "ArrowDown" ? 1 : -1
      this.#active = this.#active === -1 ? (direction > 0 ? 0 : options.length - 1) : (this.#active + direction + options.length) % options.length
      options.forEach((option, index) => option.setAttribute("aria-selected", String(index === this.#active)))
      this.searchTarget.setAttribute("aria-activedescendant", options[this.#active].id)
      options[this.#active].scrollIntoView({ block: "nearest" })
    } else if (event.key === "Enter") {
      event.preventDefault()
      options[this.#active]?.click()
    }
  }

  submit(event) {
    if (!this.#selected.size || this.#submitting) return event.preventDefault()
    this.#submitting = true
    this.startTarget.disabled = true
    this.statusTarget.textContent = "Starting conversation…"
  }

  submitted(event) {
    if (event.detail.success) return
    this.#submitting = false
    this.startTarget.disabled = !this.#selected.size
    this.statusTarget.textContent = "Couldn’t start the conversation. Please try again."
  }
}
