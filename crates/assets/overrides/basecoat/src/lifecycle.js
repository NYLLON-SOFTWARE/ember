import { applyMode, normalizeMode, preferenceKey, readMode } from "./appearance.js";
import { installPasswordToggle, preparePasswords } from "./password.js";

export function install(window, document) {
  installPasswordToggle(document);
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  let mode = readMode(window);
  let observedBody = null;
  const active = () => document.body?.dataset.styleProfile === "basecoat";

  function appearance() {
    if (!active()) return;
    applyMode(document, mode, media.matches);
    document.querySelectorAll("[data-appearance-select]").forEach(select => { select.value = mode; });
    document.querySelectorAll("[data-appearance-control]").forEach(control => { control.hidden = false; });
  }

  function stop() {
    preparePasswords(document, true);
    window.basecoat.stop();
    observedBody = null;
    // Basecoat's observer normally destroys removed nodes. A Turbo body swap happens above
    // its observation root, and cached DOM clones cannot preserve component event listeners.
    document.querySelectorAll("[data-basecoat-component]").forEach(element => {
      const name = element.dataset.basecoatComponent;
      element._destroy?.();
      delete element._destroy;
      element.removeAttribute(`data-${name}-initialized`);
      delete element.dataset.basecoatComponent;
    });
  }

  function start() {
    if (!active()) { stop(); return; }
    appearance();
    preparePasswords(document);
    if (observedBody !== document.body) {
      window.basecoat.stop();
      // A restored snapshot might originate before our before-cache cleanup ran.
      window.basecoat.initAll({ force: !!document.querySelector("[data-basecoat-component]") });
      window.basecoat.start();
      observedBody = document.body;
    } else {
      window.basecoat.initAll();
    }
  }

  document.addEventListener("change", event => {
    if (!active() || !event.target.matches?.("[data-appearance-select]")) return;
    mode = normalizeMode(event.target.value);
    try { window.localStorage.setItem(preferenceKey, mode); } catch { /* Private storage may be unavailable. */ }
    appearance();
  });
  media.addEventListener("change", () => { if (mode === "system") appearance(); });
  window.addEventListener("storage", event => {
    if (event.key === preferenceKey || event.key === null) { mode = readMode(window); appearance(); }
  });
  window.addEventListener("pageshow", start);
  document.addEventListener("turbo:before-cache", stop);
  document.addEventListener("turbo:before-render", stop);
  document.addEventListener("turbo:render", start);
  document.addEventListener("turbo:load", start);
  document.addEventListener("turbo:frame-load", start);
  document.addEventListener("turbo:before-fetch-request", event => {
    if (active()) event.detail.fetchOptions.headers["X-Ember-Style-Profile"] = "basecoat";
  });
  document.addEventListener("turbo:before-fetch-response", event => {
    if (!active()) return;
    const response = event.detail.fetchResponse.response;
    const profile = response.headers.get("X-Ember-Style-Profile")
      ?? response.headers.get("X-Matchbox-Style-Profile")
      ?? response.headers.get("X-Campfire-Style-Profile");
    const frame = event.target.closest?.("turbo-frame");
    if (frame && response.ok && profile && profile !== "basecoat" && response.headers.get("Content-Type")?.includes("text/html")) {
      // Use the final URL, after any POST redirect. Never resubmit a form to change profiles.
      event.preventDefault();
      window.location.assign(response.url);
    }
  });
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true });
  else start();
}
