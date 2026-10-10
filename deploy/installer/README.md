# Installer endpoint

`https://get.nyllon.com` is served by the `nyllon-get` Cloudflare Worker in the
Kevin Rose account, which owns the `nyllon.com` zone. Its Custom Domain binding
manages DNS and HTTPS. The `workers.dev` URL and preview URLs are disabled.

The deployed source is `worker.mjs`. `/ember` currently returns HTTP 503 with a
shell script that explains installation is unavailable and exits 1. It does not
install anything. Responses use `Cache-Control: no-store`.

Before replacing the placeholder, publish the public Ember image/release and
review and test the installer against a clean supported host. Keep the installer
under NYLLON control and review any downloaded executable dependencies.

To deploy changes with an authorized Cloudflare login:

```sh
npx wrangler deploy --config deploy/installer/wrangler.jsonc
```

The Worker can also be edited in Cloudflare's dashboard under Workers & Pages →
nyllon-get. Keep dashboard changes synchronized with these files.

Verify the HTTP status directly:

```sh
curl -sS -D - https://get.nyllon.com/ember
```

While the endpoint returns 503, `curl -f … | sh` can still report shell success
without `pipefail`, because curl suppresses the response body.
