import { Controller } from "@hotwired/stimulus"
import { put } from "@rails/request.js"

export default class extends Controller {
  static targets = [ "button", "value", "status" ]
  static values = { roomId: Number }

  async save(event) {
    event.preventDefault()
    if (this.buttonTarget.disabled) return
    this.buttonTarget.disabled = true
    this.statusTarget.hidden = true
    try {
      const response = await put(this.element.action, { body: { favorite: this.valueTarget.value }, responseKind: "json" })
      if (!response.ok) throw new Error("Favorite was not saved")
      const { favorite_channels: favorites } = await response.json
      const favorite = favorites.includes(this.roomIdValue)
      this.buttonTarget.setAttribute("aria-pressed", String(favorite))
      this.buttonTarget.title = favorite ? "Remove from favorites" : "Add to favorites"
      this.valueTarget.value = String(!favorite)
      window.Turbo?.cache.clear()
      window.dispatchEvent(new CustomEvent("ember:favorites-changed", { detail: { favorites } }))
    } catch {
      this.statusTarget.textContent = "Couldn’t save your favorite. Please try again."
      this.statusTarget.hidden = false
    } finally {
      this.buttonTarget.disabled = false
    }
  }
}
