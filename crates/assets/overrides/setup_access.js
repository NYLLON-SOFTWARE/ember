// Installer tokens travel in the fragment, never a request URL or a stored browser value.
const fragment = location.hash.slice(1)
history.replaceState(null, "", location.pathname)
const status = document.getElementById("status")
const token = new URLSearchParams(fragment).get("token")

async function openSetup() {
  if (location.protocol !== "https:" || !/^[a-f0-9]{64}$/.test(token ?? "")) {
    status.textContent = "Open the complete private HTTPS setup link printed by the installer."
    return
  }
  try {
    const response = await fetch("/first_run/access", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ token }),
      credentials: "same-origin",
      cache: "no-store",
      redirect: "manual"
    })
    if (response.status === 204) {
      location.replace("/first_run")
    } else if (response.type === "opaqueredirect") {
      location.replace("/")
    } else {
      status.textContent = "This setup link is unavailable. Request the current link from the server administrator."
    }
  } catch {
    status.textContent = "Could not open setup. Check your connection and reopen the private link."
  }
}

openSetup()
