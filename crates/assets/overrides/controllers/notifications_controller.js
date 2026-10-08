import { Controller } from "@hotwired/stimulus"
import { post } from "@rails/request.js"
import { pageIsTurboPreview } from "helpers/turbo_helpers"
import { onNextEventLoopTick } from "helpers/timing_helpers"
import { getCookie, setCookie } from "lib/cookie"

export default class extends Controller {
  static values = { subscriptionsUrl: String }
  static targets = [ "notAllowedNotice", "bell", "details", "noticeTitle", "noticeMessage", "help" ]
  static classes = [ "attention" ]

  async connect() {
    if (pageIsTurboPreview()) return
    this.#connected = true
    this.#pulseBellButton()
    try {
      if (await this.isEnabled()) {
        onNextEventLoopTick(() => { if (this.#connected) this.dispatch("ready") })
      }
    } catch {
      // A failed registration lookup must leave the bell available for an explicit retry.
    }
  }

  disconnect() {
    this.#connected = false
  }

  async attemptToSubscribe() {
    if (this.#subscribing) return
    this.#endFirstRun()
    if (!this.#allowed) {
      this.#showNotice("Notifications aren’t available here", "Open Matchbox in a browser that supports notifications. On iPhone or iPad, add it to your Home Screen first.", true)
      return
    }
    if (!this.#encodedVapidPublicKey) {
      this.#showNotice("Notifications aren’t configured", "Ask your workspace administrator to configure Web Push on this server.")
      return
    }
    if (Notification.permission === "denied") {
      this.#showNotice("Notifications are blocked", "Allow notifications for this site in your browser settings, then try again.", true)
      return
    }

    this.#subscribing = true
    const bell = this.hasBellTarget ? this.bellTarget : null
    if (bell) {
      bell.disabled = true
      bell.setAttribute("aria-busy", "true")
    }
    try {
      // Request immediately in the click handler, before awaiting worker registration. Safari
      // and other browsers require the original user activation to show the permission prompt.
      const permission = Notification.permission === "default" ? await Notification.requestPermission() : Notification.permission
      if (permission !== "granted") {
        this.#showNotice("Notifications weren’t enabled", permission === "denied"
          ? "Allow notifications for this site in your browser settings, then try again."
          : "Choose Allow in the browser prompt to receive notifications. You can try again using the bell.", permission === "denied")
        return
      }
      let registration = await this.#serviceWorkerRegistration || await this.#registerServiceWorker()
      if (!registration.active) registration = await navigator.serviceWorker.ready
      await this.#subscribe(registration)
      if (this.#connected) this.dispatch("ready")
    } catch {
      this.#showNotice("Couldn’t enable notifications", "Check your connection and try the bell again. Your room’s notification settings haven’t changed.")
    } finally {
      this.#subscribing = false
      if (bell) {
        bell.disabled = false
        bell.removeAttribute("aria-busy")
      }
    }
  }

  async isEnabled() {
    if (this.#allowed && this.#encodedVapidPublicKey && Notification.permission === "granted") {
      const registration = await this.#serviceWorkerRegistration
      const existingSubscription = await registration?.pushManager?.getSubscription()

      return Notification.permission == "granted" && registration && existingSubscription
    } else {
      return false
    }
  }

  get #allowed() {
    return window.isSecureContext && navigator.serviceWorker && window.Notification && window.PushManager
  }

  get #serviceWorkerRegistration() {
    return navigator.serviceWorker.getRegistration(window.location.origin)
  }

  #registerServiceWorker() {
    return navigator.serviceWorker.register("/service-worker.js")
  }

  #connected = false
  #subscribing = false

  #showNotice(title, message, help = false) {
    if (!this.#connected) return
    this.noticeTitleTarget.textContent = title
    this.noticeMessageTarget.textContent = message
    this.helpTarget.hidden = !help
    if (!this.notAllowedNoticeTarget.open) this.notAllowedNoticeTarget.showModal()
    this.#openSingleOption()
  }

  #openSingleOption() {
    const visibleElements = this.detailsTargets.filter(item => !this.#isHidden(item))

    if (visibleElements.length === 1) {
      this.detailsTargets.forEach(item => item.toggleAttribute("open", item === visibleElements[0]))
    }
  }

  #pulseBellButton() {
    if (this.hasBellTarget && !this.#hasSeenFirstRun) {
      this.bellTarget.classList.add(this.attentionClass)
    }
  }

  #endFirstRun() {
    if (this.hasBellTarget) this.bellTarget.classList.remove(this.attentionClass)
    this.#markFirstRunSeen()
  }

  async #subscribe(registration) {
    const existingSubscription = await registration.pushManager.getSubscription()
    const subscription = existingSubscription || await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: this.#vapidPublicKey })
    try {
      await this.#syncPushSubscription(subscription)
    } catch (error) {
      // Do not retain an unsaved new subscription: a later visit would mistake it for ready.
      if (!existingSubscription) await subscription.unsubscribe().catch(() => {})
      throw error
    }
  }

  async #syncPushSubscription(subscription) {
    const response = await post(this.subscriptionsUrlValue, { body: this.#extractJsonPayloadAsString(subscription), responseKind: "turbo-stream" })
    if (!response.ok) throw new Error("Push subscription was not saved")
  }

  get #vapidPublicKey() {
    return this.#urlBase64ToUint8Array(this.#encodedVapidPublicKey)
  }

  get #encodedVapidPublicKey() {
    return document.querySelector('meta[name="vapid-public-key"]')?.content?.trim()
  }

  get #hasSeenFirstRun() {
    if (this.#isPWA) {
      return getCookie("notifications-pwa-first-run-seen")
    } else {
      return getCookie("notifications-first-run-seen")
    }
  }

  #markFirstRunSeen = (event) => {
    if (this.#isPWA) {
      setCookie("notifications-pwa-first-run-seen", true)
    } else {
      setCookie("notifications-first-run-seen", true)
    }
  }

  #extractJsonPayloadAsString(subscription) {
    const { endpoint, keys: { p256dh, auth } } = subscription.toJSON()
    return JSON.stringify({ push_subscription: { endpoint, p256dh_key: p256dh, auth_key: auth } })
  }

  // VAPID public key comes encoded as base64 but service worker registration needs it as a Uint8Array
  #urlBase64ToUint8Array(base64String) {
    const padding = "=".repeat((4 - base64String.length % 4) % 4)
    const base64 = (base64String + padding).replace(/-/g, "+").replace(/_/g, "/")

    const rawData = window.atob(base64)
    const outputArray = new Uint8Array(rawData.length)

    for (let i = 0; i < rawData.length; ++i) {
      outputArray[i] = rawData.charCodeAt(i)
    }

    return outputArray
  }

  #isHidden(item) {
    return (item.offsetParent === null)
  }

  get #isPWA() {
    return window.matchMedia("(display-mode: standalone)").matches
  }
}
