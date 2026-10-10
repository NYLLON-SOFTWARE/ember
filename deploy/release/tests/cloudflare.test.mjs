import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import { manageDeployment, uploadedVersion } from '../cloudflare.mjs'

const previousDeployment = '11111111-1111-1111-1111-111111111111'
const candidateDeployment = '22222222-2222-2222-2222-222222222222'
const newerDeployment = '33333333-3333-3333-3333-333333333333'
const previousVersion = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
const candidateVersion = 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb'
const newerVersion = 'cccccccc-cccc-cccc-cccc-cccccccccccc'
const message = 'Ember release run 123456 attempt 2'
const previous = deployment(previousDeployment, previousVersion)
const candidate = deployment(candidateDeployment, candidateVersion, message)
const newer = deployment(newerDeployment, newerVersion, 'Ember release run 999999 attempt 1')

function deployment(id, version, marker = '') {
  return { id, created_on: '2026-10-10T00:00:00Z', strategy: 'percentage',
    versions: [{ version_id: version, percentage: 100 }], annotations: { 'workers/message': marker } }
}

function mockApi(deployments, subdomain = { enabled: false, previews_enabled: false }) {
  const calls = []
  const api = async (options, resource = 'deployments') => {
    calls.push({ ...(options ?? { method: 'GET' }), resource })
    if (resource === 'subdomain') return subdomain
    if (options?.method === 'POST') return deployment(newerDeployment, previousVersion)
    return { deployments }
  }
  return { api, calls }
}

const restoreEnv = { PREVIOUS_VERSION: previousVersion, CANDIDATE_DEPLOYMENT: candidateDeployment }
const candidateEnv = { PREVIOUS_DEPLOYMENT: previousDeployment, CANDIDATE_VERSION: candidateVersion, DEPLOYMENT_MESSAGE: message }
const silent = () => {}

test('previous captures active deployment ID and version, preserving the API active-first order', async () => {
  const { api } = mockApi([{ ...previous, created_on: '2026-01-01T00:00:00Z' }, newer])
  const output = {}
  await manageDeployment('previous', {}, api, (key, value) => { output[key] = value }, silent)
  assert.deepEqual(output, { version: previousVersion, deployment: previousDeployment })
})

test('release preflight refuses enabled or unknown alternate URLs before inspecting or changing deployments', async () => {
  for (const subdomain of [
    { enabled: true, previews_enabled: false },
    { enabled: false, previews_enabled: true },
    { enabled: false },
    {}
  ]) {
    const { api, calls } = mockApi([previous], subdomain)
    await assert.rejects(manageDeployment('previous', {}, api, silent, silent), /Disable workers.dev and preview URLs/)
    assert.deepEqual(calls, [{ method: 'GET', resource: 'subdomain' }])
  }
})

test('post-attempt capture identifies an activated candidate even when its deploy command failed', async () => {
  const { api, calls } = mockApi([candidate, previous])
  const output = {}
  await manageDeployment('candidate', candidateEnv, api, (key, value) => { output[key] = value }, silent)
  assert.deepEqual(output, { deployment: candidateDeployment })
  assert.equal(calls.length, 1)
})

test('failure before activation records no candidate and preserves the previous installer', async () => {
  const { api, calls } = mockApi([previous])
  const output = {}
  await manageDeployment('candidate', candidateEnv, api, (key, value) => { output[key] = value }, silent)
  assert.deepEqual(output, { deployment: '' })
  await manageDeployment('restore', { PREVIOUS_VERSION: previousVersion }, api, silent, silent)
  assert.ok(calls.every(call => call.method === 'GET'))
})

test('unchanged previous deployment records no candidate when upload produced no version ID', async () => {
  const { api, calls } = mockApi([previous])
  const output = {}
  await manageDeployment('candidate', { ...candidateEnv, CANDIDATE_VERSION: '' }, api,
    (key, value) => { output[key] = value }, silent)
  assert.deepEqual(output, { deployment: '' })
  assert.equal(calls.length, 1)
})

test('capture refuses to claim a deployment from a different run or attempt', async () => {
  for (const current of [newer, deployment(candidateDeployment, candidateVersion, 'Ember release run 123456 attempt 1')]) {
    const { api, calls } = mockApi([current])
    await assert.rejects(manageDeployment('candidate', candidateEnv, api, silent, silent), /different operation/)
    assert.ok(calls.every(call => call.method === 'GET'))
  }
})

test('candidate capture requires the exact uploaded version even when another version uses this runstamp', async () => {
  const { api, calls } = mockApi([deployment(candidateDeployment, newerVersion, message)])
  await assert.rejects(manageDeployment('candidate', candidateEnv, api, silent, silent), /different operation/)
  assert.equal(calls.length, 1)
  for (const version of [undefined, '', 'not-a-uuid']) {
    await assert.rejects(manageDeployment('candidate', { ...candidateEnv, CANDIDATE_VERSION: version }, api, silent, silent), /attempt identity/)
  }
})

test('first release failure can restore the exact previous HTTP503 Worker version', async () => {
  const { api, calls } = mockApi([candidate, previous])
  await manageDeployment('restore', restoreEnv, api, silent, silent)
  assert.equal(calls.length, 2)
  assert.equal(calls[0].method, 'GET')
  assert.equal(calls[1].method, 'POST')
  const body = JSON.parse(calls[1].body)
  assert.deepEqual(body.versions, [{ version_id: previousVersion, percentage: 100 }])
  assert.match(body.annotations['workers/message'], new RegExp(candidateDeployment))
})

test('rollback recovers a lost candidate ID only from the saved original attempt and uploaded version', async () => {
  const { api, calls } = mockApi([candidate, previous])
  await manageDeployment('restore', { ...candidateEnv, PREVIOUS_VERSION: previousVersion }, api, silent, silent)
  assert.equal(calls.length, 2)
  assert.deepEqual(JSON.parse(calls[1].body).versions, [{ version_id: previousVersion, percentage: 100 }])
})

test('lost-ID recovery refuses another deployment, changed run or attempt, or incomplete saved identity', async () => {
  const recovery = { ...candidateEnv, PREVIOUS_VERSION: previousVersion }
  for (const [current, env] of [
    [newer, recovery],
    [deployment(newerDeployment, candidateVersion, 'Ember release run 123456 attempt 3'), recovery],
    [deployment(newerDeployment, newerVersion, message), recovery],
    [candidate, { ...recovery, PREVIOUS_DEPLOYMENT: '' }],
    [candidate, { ...recovery, CANDIDATE_VERSION: '' }],
    [candidate, { ...recovery, DEPLOYMENT_MESSAGE: '' }],
    [candidate, { ...recovery, CANDIDATE_DEPLOYMENT: 'malformed' }]
  ]) {
    const { api, calls } = mockApi([current])
    await assert.rejects(manageDeployment('restore', env, api, silent, silent), /stale rollback/)
    assert.equal(calls.length, 1)
  }
})

test('retrying an old failed public gate cannot roll back a newer deployment', async () => {
  const { api, calls } = mockApi([newer, candidate, previous])
  await assert.rejects(manageDeployment('restore', restoreEnv, api, silent, silent), /stale rollback/)
  assert.equal(calls.length, 1)
})

test('deployment identity prevents rollback even if another deployment uses the candidate version', async () => {
  const { api, calls } = mockApi([deployment(newerDeployment, candidateVersion)])
  await assert.rejects(manageDeployment('restore', restoreEnv, api, silent, silent), /stale rollback/)
  assert.equal(calls.length, 1)
})

test('retrying a completed rollback is a harmless no-op despite its new deployment ID', async () => {
  const { api, calls } = mockApi([deployment(newerDeployment, previousVersion), candidate])
  await manageDeployment('restore', restoreEnv, api, silent, silent)
  assert.equal(calls.length, 1)
})

test('missing or malformed candidate identity cannot authorize replacement', async () => {
  for (const candidateId of [undefined, '', 'not-a-uuid']) {
    const { api, calls } = mockApi([candidate])
    await assert.rejects(manageDeployment('restore', { ...restoreEnv, CANDIDATE_DEPLOYMENT: candidateId }, api, silent, silent), /stale rollback/)
    assert.equal(calls.length, 1)
  }
})

test('split deployments and unverifiable active deployment IDs fail without mutation', async () => {
  for (const current of [{ ...candidate, id: 'unknown' }, { ...candidate, versions: [
    { version_id: candidateVersion, percentage: 50 }, { version_id: previousVersion, percentage: 50 }
  ] }]) {
    const { api, calls } = mockApi([current])
    await assert.rejects(manageDeployment('restore', restoreEnv, api, silent, silent), /single recoverable/)
    assert.equal(calls.length, 1)
  }
})

function uploadOutput(version = candidateVersion, extra = {}) {
  // Shape written by pinned Wrangler's version-upload record, including its output timestamp.
  return JSON.stringify({ type: 'version-upload', version: 1, worker_name: 'nyllon-get',
    worker_tag: 'tag', version_id: version, targets: [], timestamp: '2026-10-10T00:00:00Z', ...extra }) + '\n'
}

test('structured upload output identifies one exact Worker version among other record types', () => {
  const unrelated = JSON.stringify({ type: 'other-output', version: 1 }) + '\n'
  assert.equal(uploadedVersion(unrelated + uploadOutput() + '\n'), candidateVersion)
})

test('upload output refuses malformed, missing, ambiguous, or mismatched artifact identities', () => {
  for (const contents of ['', 'null\n', '{}\n', uploadOutput(null), uploadOutput('not-a-uuid'),
    uploadOutput(candidateVersion, { worker_name: 'another-worker' }),
    uploadOutput(candidateVersion, { version: 2 }), uploadOutput() + uploadOutput(newerVersion)]) {
    assert.throws(() => uploadedVersion(contents), /one verified nyllon-get version upload/)
  }
  assert.throws(() => uploadedVersion('not JSON\n'), /Malformed Wrangler upload output/)
})

test('uploaded CLI emits only the validated UUID and works without Cloudflare credentials or API calls', t => {
  const directory = mkdtempSync(join(tmpdir(), 'ember-upload-output-'))
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  const source = join(directory, 'upload.ndjson')
  const output = join(directory, 'github-output')
  writeFileSync(source, uploadOutput())
  const result = spawnSync(process.execPath, [new URL('../cloudflare.mjs', import.meta.url).pathname, 'uploaded'], {
    encoding: 'utf8', env: { GITHUB_OUTPUT: output, WRANGLER_OUTPUT_FILE_PATH: source }
  })
  assert.equal(result.status, 0, result.stderr)
  assert.equal(result.stdout, '')
  assert.equal(result.stderr, '')
  assert.equal(readFileSync(output, 'utf8'), `version=${candidateVersion}\n`)
})

test('uploaded CLI cannot pass an unverified artifact UUID into the activation step', t => {
  const directory = mkdtempSync(join(tmpdir(), 'ember-upload-output-'))
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  const source = join(directory, 'upload.ndjson')
  const output = join(directory, 'github-output')
  writeFileSync(source, uploadOutput(candidateVersion, { worker_name: 'another-worker' }))
  writeFileSync(output, '')
  const result = spawnSync(process.execPath, [new URL('../cloudflare.mjs', import.meta.url).pathname, 'uploaded'], {
    encoding: 'utf8', env: { GITHUB_OUTPUT: output, WRANGLER_OUTPUT_FILE_PATH: source }
  })
  assert.notEqual(result.status, 0)
  assert.equal(readFileSync(output, 'utf8'), '')
})

test('CLI recovers after a failed candidate lookup and a retried job cannot replace a newer installer', t => {
  const directory = mkdtempSync(join(tmpdir(), 'ember-cloudflare-recovery-'))
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  const state = join(directory, 'state.json')
  const requests = join(directory, 'requests.ndjson')
  const preload = join(directory, 'mock-cloudflare.mjs')
  writeFileSync(state, JSON.stringify(candidate))
  writeFileSync(requests, '')
  // Exercise the real CLI and its endpoint handling without opening sockets or using credentials.
  writeFileSync(preload, `import { appendFileSync, readFileSync, writeFileSync } from 'node:fs'
globalThis.fetch = async (url, options = {}) => {
  if (!url.endsWith('/workers/scripts/nyllon-get/deployments')) throw new Error('Unexpected API resource')
  appendFileSync(process.env.MOCK_REQUESTS, JSON.stringify({ method: options.method ?? 'GET' }) + '\\n')
  if (process.env.MOCK_CAPTURE_FAILURE) return new Response(JSON.stringify({ success: false }), { status: 503 })
  const current = JSON.parse(readFileSync(process.env.MOCK_STATE, 'utf8'))
  if (options.method === 'POST') {
    const body = JSON.parse(options.body)
    writeFileSync(process.env.MOCK_STATE, JSON.stringify({ ...current, versions: body.versions }))
    return new Response(JSON.stringify({ success: true, result: {} }))
  }
  return new Response(JSON.stringify({ success: true, result: { deployments: [current] } }))
}
`)
  const run = (command, extra = {}) => spawnSync(process.execPath,
    ['--import', preload, new URL('../cloudflare.mjs', import.meta.url).pathname, command], {
      encoding: 'utf8', env: { CLOUDFLARE_API_TOKEN: 'test-scoped-token',
        CLOUDFLARE_ACCOUNT_ID: 'b9a05c0456567559ed575566fc5bb3f3',
        GITHUB_OUTPUT: join(directory, 'github-output'), MOCK_STATE: state, MOCK_REQUESTS: requests,
        ...candidateEnv, PREVIOUS_VERSION: previousVersion, ...extra }
    })
  const capture = run('candidate', { MOCK_CAPTURE_FAILURE: '1' })
  assert.notEqual(capture.status, 0)
  assert.match(capture.stderr, /HTTP 503/)
  // The rerun has a new GitHub attempt number, but recovery must use the saved original message.
  const recovery = run('restore', { GITHUB_RUN_ATTEMPT: '3', CANDIDATE_DEPLOYMENT: '' })
  assert.equal(recovery.status, 0, recovery.stderr)
  assert.equal(JSON.parse(readFileSync(state, 'utf8')).versions[0].version_id, previousVersion)
  writeFileSync(state, JSON.stringify(newer))
  const retry = run('restore', { GITHUB_RUN_ATTEMPT: '4', CANDIDATE_DEPLOYMENT: '' })
  assert.notEqual(retry.status, 0)
  assert.match(retry.stderr, /stale rollback/)
  assert.equal(JSON.parse(readFileSync(state, 'utf8')).id, newerDeployment)
  const calls = readFileSync(requests, 'utf8').trim().split('\n').map(line => JSON.parse(line))
  assert.equal(calls.filter(call => call.method === 'POST').length, 1)
})
