import test from 'node:test'
import assert from 'node:assert/strict'
import worker, { serve } from '../worker.mjs'
const request = (path, method = 'GET') => new Request('https://get.nyllon.com' + path, { method })
test('unreleased bootstrap fails closed and is never cached', async () => {
  const response = serve(request('/ember'), null)
  assert.equal(response.status, 503)
  assert.equal(response.headers.get('Cache-Control'), 'no-store')
  assert.match(await response.text(), /exit 1/)
  assert.equal(serve(request('/releases/stable.json'), null).status, 503)
})
test('only exact published artifacts are served', async () => {
  const release = { bootstrap: '#!/bin/sh\nexit 0\n', manifest: { version: '1.0.0' }, guide: '<h1>Install Ember</h1>' }
  assert.equal(await serve(request('/ember'), release).text(), release.bootstrap)
  assert.equal(await serve(request('/'), release).text(), release.guide)
  assert.deepEqual(await serve(request('/releases/stable.json'), release).json(), release.manifest)
  assert.equal(await serve(request('/ember', 'HEAD'), release).text(), '')
  assert.equal(serve(request('/ember', 'POST'), release).status, 405)
  assert.equal(serve(request('/nope'), release).status, 404)
})

test('Cloudflare bindings cannot replace the gated publication', async () => {
  const response = worker.fetch(request('/ember'), {}, {})
  assert.equal(response.status, 503)
  assert.match(await response.text(), /exit 1/)
})
