import { appendFileSync, readFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'

const uuid = /^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/
const deploymentMessage = /^Ember release run [0-9]+ attempt [0-9]+$/

// Wrangler 4.149.0 writes one JSON record per line to WRANGLER_OUTPUT_FILE_PATH.
// Its version-upload record identifies the exact artifact, without parsing human-readable logs.
export function uploadedVersion(contents) {
  let records
  try {
    records = contents.split('\n').filter(line => line.trim()).map(line => JSON.parse(line))
  } catch {
    throw new Error('Malformed Wrangler upload output')
  }
  const uploads = records.filter(record => record?.type === 'version-upload')
  const upload = uploads[0]
  if (uploads.length !== 1 || upload.version !== 1 || upload.worker_name !== 'nyllon-get' || !uuid.test(upload.version_id ?? '')) {
    throw new Error('Expected exactly one verified nyllon-get version upload')
  }
  return upload.version_id
}

async function currentDeployment(api) {
  const { deployments } = await api()
  // Cloudflare defines the first list entry as the deployment actively serving traffic.
  // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/list/
  const current = deployments?.[0]
  if (!uuid.test(current?.id ?? '') || current?.versions?.length !== 1 || current.versions[0].percentage !== 100 || !uuid.test(current.versions[0].version_id)) {
    throw new Error('Expected a single recoverable current Worker deployment; resolve split deployments before releasing')
  }
  return current
}

function producedByAttempt(current, env) {
  return uuid.test(env.CANDIDATE_VERSION ?? '') && deploymentMessage.test(env.DEPLOYMENT_MESSAGE ?? '') &&
    current.versions[0].version_id === env.CANDIDATE_VERSION && current.annotations?.['workers/message'] === env.DEPLOYMENT_MESSAGE
}

export async function manageDeployment(command, env, api, output, log = console.log) {
  if (command === 'previous') {
    // Versions commands preserve triggers. Require the existing Worker to have no alternate URLs.
    const subdomain = await api(undefined, 'subdomain')
    if (subdomain?.enabled !== false || subdomain?.previews_enabled !== false) {
      throw new Error('Disable workers.dev and preview URLs for nyllon-get before releasing')
    }
    const current = await currentDeployment(api)
    output('version', current.versions[0].version_id)
    output('deployment', current.id)
  } else if (command === 'candidate') {
    if (!uuid.test(env.PREVIOUS_DEPLOYMENT ?? '') || !deploymentMessage.test(env.DEPLOYMENT_MESSAGE ?? '')) {
      throw new Error('Missing verified deployment attempt identity')
    }
    const current = await currentDeployment(api)
    if (current.id === env.PREVIOUS_DEPLOYMENT) {
      output('deployment', '')
      log('The deployment attempt did not replace the previous installer.')
      return
    }
    if (!uuid.test(env.CANDIDATE_VERSION ?? '')) throw new Error('Missing verified deployment attempt identity')
    if (!producedByAttempt(current, env)) {
      throw new Error('Current Worker deployment belongs to a different operation; refusing to claim it for recovery')
    }
    output('deployment', current.id)
  } else if (command === 'restore') {
    const version = env.PREVIOUS_VERSION
    if (!uuid.test(version ?? '')) throw new Error('Missing verified previous Worker version')
    const current = await currentDeployment(api)
    if (current.versions[0].version_id === version) {
      log('Previous installer is already active. No rollback needed; website promotion withheld.')
      return
    }
    const candidate = env.CANDIDATE_DEPLOYMENT
    if (candidate) {
      if (!uuid.test(candidate) || current.id !== candidate) {
        throw new Error('Current Worker deployment differs from this release attempt; refusing a stale rollback')
      }
    } else if (!uuid.test(env.PREVIOUS_DEPLOYMENT ?? '') || current.id === env.PREVIOUS_DEPLOYMENT || !producedByAttempt(current, env)) {
      // A failed post-activation lookup can lose the ID. Only the saved original attempt and
      // exact uploaded version can recover it; retrying another job must not use its new attempt.
      throw new Error('Current Worker deployment differs from this release attempt; refusing a stale rollback')
    }
    await api({ method: 'POST', body: JSON.stringify({ strategy: 'percentage', versions: [{ version_id: version, percentage: 100 }], annotations: { 'workers/message': `Restore previous installer after failure of deployment ${current.id}` } }) })
    log('Previous installer restored. Website promotion withheld.')
  } else {
    throw new Error('Expected previous, candidate, or restore')
  }
}

async function main() {
  const output = (key, value) => appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`)
  if (process.argv[2] === 'uploaded') {
    const contents = readFileSync(process.env.WRANGLER_OUTPUT_FILE_PATH, 'utf8')
    output('version', uploadedVersion(contents))
    return
  }
  const account = process.env.CLOUDFLARE_ACCOUNT_ID
  const token = process.env.CLOUDFLARE_API_TOKEN
  if (account !== 'b9a05c0456567559ed575566fc5bb3f3' || !token) throw new Error('A scoped Cloudflare release credential is required')
  const endpoint = `https://api.cloudflare.com/client/v4/accounts/${account}/workers/scripts/nyllon-get`
  const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }
  const api = async (options, resource = 'deployments') => {
    const response = await fetch(`${endpoint}/${resource}`, { headers, signal: AbortSignal.timeout(30_000), ...options })
    const result = await response.json()
    if (!response.ok || !result.success) throw new Error(`Cloudflare deployment operation failed (HTTP ${response.status})`)
    return result.result
  }
  await manageDeployment(process.argv[2], process.env, api, output)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main()
