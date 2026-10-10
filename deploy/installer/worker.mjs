import { publication } from "./published.mjs"

const unavailableInstaller = `#!/bin/sh
printf '%s\\n' 'Ember installation is not available yet. A public release and installer will be published here.' >&2
exit 1
`

export function serve(request, release = publication) {
  const { pathname } = new URL(request.url)
  const headers = {
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
    "Referrer-Policy": "no-referrer",
    "Content-Type": "text/plain; charset=utf-8"
  }
  const respond = (body, status = 200, extra = {}) => new Response(request.method === "HEAD" ? null : body, {
    status, headers: { ...headers, ...extra }
  })
  if (request.method !== "GET" && request.method !== "HEAD") return respond("Method not allowed\n", 405, { Allow: "GET, HEAD" })
  if (pathname === "/ember" || pathname === "/ember/") return respond(release?.bootstrap ?? unavailableInstaller, release ? 200 : 503)
  if (pathname === "/releases/stable.json") return release ? respond(JSON.stringify(release.manifest) + "\n", 200, { "Content-Type": "application/json; charset=utf-8" }) : respond("No stable release yet\n", 503)
  if (pathname === "/") return release ? respond(release.guide, 200, {
    "Content-Type": "text/html; charset=utf-8",
    "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'"
  }) : respond("NYLLON installers\n\nEmber installation will be available at /ember after its first public release.\n")
  return respond("Not found\n", 404)
}
export default { fetch(request) { return serve(request) } }
