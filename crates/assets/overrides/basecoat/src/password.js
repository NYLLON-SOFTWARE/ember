function setVisibility(button, input, visible) {
  input.type = visible ? "text" : "password";
  button.setAttribute("aria-pressed", String(visible));
  button.setAttribute("aria-label", visible ? "Hide password" : "Show password");
}

export function preparePasswords(document, conceal = false) {
  document.querySelectorAll("[data-password-toggle]").forEach(button => {
    const input = document.getElementById(button.getAttribute("aria-controls"));
    if (!input) return;
    button.hidden = false;
    setVisibility(button, input, !conceal && input.type === "text");
  });
}

export function installPasswordToggle(document) {
  // Delegation survives Turbo's body swaps without attaching listeners to cached controls.
  document.addEventListener("click", event => {
    const button = event.target.closest?.("[data-password-toggle]");
    if (!button) return;
    const input = document.getElementById(button.getAttribute("aria-controls"));
    if (input) setVisibility(button, input, input.type === "password");
  });
}
