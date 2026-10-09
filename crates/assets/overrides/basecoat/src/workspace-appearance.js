import { applyMode, normalizeMode, preferenceKey, readMode } from "./appearance.js";

// Run before CSS to share onboarding's saved preference without a light-mode flash.
const media = window.matchMedia("(prefers-color-scheme: dark)");
let mode = readMode(window);
applyMode(document, mode, media.matches);

if (!window.matchboxAppearance) {
  window.matchboxAppearance = true;
  const active = () => document.body?.classList.contains("mb-app");
  const menu = () => document.querySelector("#appearance-menu");

  function refresh() {
    if (!active()) return;
    applyMode(document, mode, media.matches);
    document.querySelectorAll("[data-workspace-appearance]").forEach(control => { control.hidden = false; });
    document.querySelectorAll("[data-appearance-icon]").forEach(icon => { icon.hidden = icon.dataset.appearanceIcon !== mode; });
    document.querySelectorAll('#appearance-menu input').forEach(input => { input.checked = input.value === mode; });
    const trigger = document.querySelector(".mb-appearance-toggle");
    const label = `Appearance: ${mode[0].toUpperCase()}${mode.slice(1)}`;
    trigger?.setAttribute("aria-label", label);
    trigger?.setAttribute("title", label);
    document.querySelectorAll('meta[name="theme-color"]').forEach(meta => {
      meta.content = document.documentElement.classList.contains("dark") ? "#171717" : "#ffffff";
    });
  }

  document.addEventListener("change", event => {
    if (!active() || !event.target.matches?.('#appearance-menu input')) return;
    mode = normalizeMode(event.target.value);
    try { window.localStorage.setItem(preferenceKey, mode); } catch { /* The choice still works when storage is unavailable. */ }
    refresh();
  });
  media.addEventListener("change", () => { if (mode === "system") refresh(); });
  window.addEventListener("storage", event => {
    if (event.key === preferenceKey || event.key === "campfire:appearance" || event.key === null) {
      mode = readMode(window);
      refresh();
    }
  });
  document.addEventListener("turbo:before-cache", () => { if (menu()?.matches(":popover-open")) menu().hidePopover(); });
  document.addEventListener("turbo:render", refresh);
  document.addEventListener("turbo:load", refresh);
  window.addEventListener("pageshow", refresh);
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", refresh, { once: true });
  else refresh();
}
