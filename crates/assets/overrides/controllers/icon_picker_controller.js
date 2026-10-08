import { Controller } from "@hotwired/stimulus"

const catalogs = new Map()
const pageSize = 72
const favorites = ["hash", "messages-square", "bot", "code", "sparkles", "coffee", "rocket", "palette", "megaphone", "heart", "lightbulb", "music", "gamepad-2", "book-open", "camera", "calendar", "globe", "users", "flame", "leaf", "star", "wrench", "shield", "lock"]

// Digested catalogs are immutable and shared across Turbo visits. The full library is fetched
// only after opening the picker; normal channel pages render their single SVG on the server.
function loadCatalog(url) {
  if (!catalogs.has(url)) {
    const pending = fetch(url, { credentials: "same-origin" })
      .then(response => {
        if (!response.ok) throw new Error("Icon catalog could not be loaded")
        return response.json()
      })
      .catch(error => { catalogs.delete(url); throw error })
    catalogs.set(url, pending)
  }
  return catalogs.get(url)
}

export default class extends Controller {
  static targets = ["fallback", "enhanced", "input", "trigger", "preview", "label", "dialog", "search", "browse", "default", "results", "grid", "more", "retry", "selectionPreview", "selectionLabel", "apply"]
  static values = { catalogUrl: String }

  #catalog
  #matches = []
  #visible = pageSize
  #pending = ""
  #events
  #opening = 0

  connect() {
    if (typeof this.dialogTarget.showModal !== "function") return
    this.fallbackTarget.hidden = true
    this.enhancedTarget.hidden = false
    this.#events = new AbortController()
    document.addEventListener("turbo:before-cache", () => this.cancel(), { signal: this.#events.signal })
  }

  disconnect() {
    this.#events?.abort()
    this.#opening++
    if (this.hasDialogTarget && this.dialogTarget.open) this.dialogTarget.close()
  }

  async open() {
    this.#pending = this.inputTarget.value
    this.searchTarget.value = ""
    this.selectionPreviewTarget.replaceChildren(...[...this.previewTarget.childNodes].map(node => node.cloneNode(true)))
    this.selectionLabelTarget.textContent = this.labelTarget.textContent
    this.defaultTarget.setAttribute("aria-pressed", String(!this.#pending))
    this.dialogTarget.showModal()
    this.searchTarget.focus()
    this.browseTarget.scrollTop = 0
    await this.#load()
  }

  async #load() {
    const opening = ++this.#opening
    this.retryTarget.hidden = true
    this.resultsTarget.textContent = "Loading icons…"
    this.gridTarget.setAttribute("aria-busy", "true")
    try {
      this.#catalog ||= await loadCatalog(this.catalogUrlValue)
      if (!this.element.isConnected || !this.dialogTarget.open || opening !== this.#opening) return
      this.search()
    } catch {
      if (!this.element.isConnected || !this.dialogTarget.open || opening !== this.#opening) return
      this.resultsTarget.textContent = "Icons couldn’t load. Try again; your current icon is unchanged."
      this.retryTarget.hidden = false
      this.moreTarget.hidden = true
    } finally {
      if (this.element.isConnected && opening === this.#opening) this.gridTarget.removeAttribute("aria-busy")
    }
  }

  retry() { return this.#load() }

  search() {
    if (!this.#catalog) return
    const terms = this.searchTarget.value.trim().toLowerCase().split(/[\s-]+/).filter(Boolean)
    this.#matches = this.#catalog.filter(icon => terms.every(term => `${icon.name} ${icon.label}`.toLowerCase().includes(term)))
    if (!terms.length) {
      const popular = new Map(favorites.map((name, index) => [name, index]))
      this.#matches.sort((a, b) => (popular.get(a.name) ?? favorites.length) - (popular.get(b.name) ?? favorites.length))
    }
    this.#visible = pageSize
    this.browseTarget.scrollTop = 0
    this.#render()
  }

  #render() {
    const visible = this.#matches.slice(0, this.#visible)
    const fragment = document.createDocumentFragment()
    const active = visible.find(icon => icon.name === this.#pending)?.name || visible[0]?.name
    for (const icon of visible) {
      const button = document.createElement("button")
      button.type = "button"
      button.className = "mb-icon-option"
      button.dataset.icon = icon.name
      button.title = icon.label
      button.tabIndex = icon.name === active ? 0 : -1
      button.setAttribute("aria-label", `Use ${icon.label} icon`)
      button.setAttribute("aria-pressed", String(icon.name === this.#pending))
      const drawing = document.createElement("span")
      drawing.setAttribute("aria-hidden", "true")
      // This is generated, validated geometric SVG from our embedded Lucide asset, not uploads.
      drawing.innerHTML = icon.svg
      const label = document.createElement("span")
      label.textContent = icon.label
      button.append(drawing, label)
      fragment.append(button)
    }
    this.gridTarget.replaceChildren(fragment)
    this.moreTarget.hidden = visible.length >= this.#matches.length
    const count = this.#matches.length
    this.resultsTarget.textContent = count
      ? `${count.toLocaleString()} ${count === 1 ? "icon" : "icons"}${count > visible.length ? ` · ${visible.length} shown` : ""}`
      : "No icons match your search. Try another word."
  }

  more() {
    const previous = Math.min(this.#visible, this.#matches.length)
    this.#visible += pageSize
    this.#render()
    const next = this.gridTarget.children[previous]
    if (next) this.#focus(next)
  }

  choose(event) {
    const button = event.target.closest("button[data-icon]")
    if (!button || !this.gridTarget.contains(button)) return
    this.#select(button.dataset.icon)
  }

  default() { this.#select("") }

  #select(name) {
    const icon = this.#catalog?.find(icon => icon.name === name)
    if (name && !icon) return
    this.#pending = name
    this.selectionPreviewTarget.innerHTML = icon?.svg || "#"
    this.selectionLabelTarget.textContent = icon?.label || "Default hashtag"
    this.defaultTarget.setAttribute("aria-pressed", String(!name))
    for (const button of this.gridTarget.children) button.setAttribute("aria-pressed", String(button.dataset.icon === name))
  }

  selectFirst() {
    if (this.#matches[0]) this.#select(this.#matches[0].name)
  }

  navigate(event) {
    const button = event.target.closest("button[data-icon]")
    if (!button) return
    const buttons = [...this.gridTarget.children]
    const index = buttons.indexOf(button)
    const columns = getComputedStyle(this.gridTarget).gridTemplateColumns.split(" ").length
    const movement = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -columns, ArrowDown: columns }
    let next
    if (event.key === "Home") next = 0
    else if (event.key === "End") next = buttons.length - 1
    else if (event.key in movement) next = Math.max(0, Math.min(buttons.length - 1, index + movement[event.key]))
    else return
    event.preventDefault()
    this.#focus(buttons[next])
  }

  #focus(button) {
    for (const item of this.gridTarget.children) item.tabIndex = item === button ? 0 : -1
    button.focus()
  }

  apply() {
    this.inputTarget.value = this.#pending
    this.previewTarget.replaceChildren(...[...this.selectionPreviewTarget.childNodes].map(node => node.cloneNode(true)))
    this.labelTarget.textContent = this.selectionLabelTarget.textContent
    this.inputTarget.dispatchEvent(new Event("change", { bubbles: true }))
    this.dialogTarget.close()
  }

  cancel(event) {
    event?.preventDefault()
    this.#opening++
    if (this.dialogTarget.open) this.dialogTarget.close()
  }

  closed() {
    this.#opening++
    if (this.element.isConnected) this.triggerTarget.focus({ preventScroll: true })
  }

  backdrop(event) {
    if (event.target !== this.dialogTarget) return
    const rect = this.dialogTarget.getBoundingClientRect()
    if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) this.cancel()
  }
}
