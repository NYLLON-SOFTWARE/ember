import assert from 'node:assert/strict'
import { test } from 'node:test'
import { manageDeployment } from '../cloudflare.mjs'

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

function mockApi(deployments) {
  const calls = []
  const api = async options => {
    calls.push(options ?? { method: 'GET' })
    if (options?.method === 'POST') return deployment(newerDeployment, previousVersion)
    return { deployments }
  }
  return { api, calls }
}

const restoreEnv = { PREVIOUS_VERSION: previousVersion, CANDIDATE_DEPLOYMENT: candidateDeployment }
const candidateEnv = { PREVIOUS_DEPLOYMENT: previousDeployment, DEPLOYMENT_MESSAGE: message }
const silent = () => {}

test('previous captures active deployment ID and version, preserving the API active-first order', async () => {
  const { api } = mockApi([{ ...previous, created_on: '2026-01-01T00:00:00Z' }, newer])
  const output = {}
  await manageDeployment('previous', {}, api, (key, value) => { output[key] = value }, silent)
  assert.deepEqual(output, { version: previousVersion, deployment: previousDeployment })
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

test('capture refuses to claim a deployment from a different run or attempt', async () => {
  for (const current of [newer, deployment(candidateDeployment, candidateVersion, 'Ember release run 123456 attempt 1')]) {
    const { api, calls } = mockApi([current])
    await assert.rejects(manageDeployment('candidate', candidateEnv, api, silent, silent), /different operation/)
    assert.ok(calls.every(call => call.method === 'GET'))
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
