export const preferenceKey = "matchbox:appearance";

export function normalizeMode(mode) {
  return ["light", "dark", "system"].includes(mode) ? mode : "system";
}

export function readMode(window) {
  try { return normalizeMode(window.localStorage.getItem(preferenceKey) ?? window.localStorage.getItem("campfire:appearance")); }
  catch { return "system"; }
}

export function applyMode(document, mode, systemDark) {
  const dark = mode === "dark" || (mode === "system" && systemDark);
  document.documentElement.classList.toggle("dark", dark);
  document.documentElement.dataset.appearance = mode;
  document.documentElement.style.colorScheme = dark ? "dark" : "light";
}
