const unavailableInstaller = `#!/bin/sh
printf '%s\\n' 'Ember installation is not available yet. A public release and installer will be published here.' >&2
exit 1
`;

export default {
  fetch(request) {
    const { pathname } = new URL(request.url);
    const headers = {
      "Content-Type": "text/plain; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    };
    const respond = (body, status) =>
      new Response(request.method === "HEAD" ? null : body, { status, headers });

    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method not allowed\n", {
        status: 405,
        headers: { ...headers, Allow: "GET, HEAD" },
      });
    }

    if (pathname === "/ember" || pathname === "/ember/") {
      // Keep shell pipelines safe while the public image and reviewed installer are pending.
      return respond(unavailableInstaller, 503);
    }
    if (pathname === "/") {
      return respond("NYLLON installers\n\nEmber installation will be available at /ember after its first public release.\n", 200);
    }
    return respond("Not found\n", 404);
  },
};
