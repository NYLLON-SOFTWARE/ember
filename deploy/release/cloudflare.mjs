import { appendFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'

const uuid = /^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/

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

export async function manageDeployment(command, env, api, output, log = console.log) {
  if (command === 'previous') {
    const current = await currentDeployment(api)
    output('version', current.versions[0].version_id)
    output('deployment', current.id)
  } else if (command === 'candidate') {
    if (!uuid.test(env.PREVIOUS_DEPLOYMENT ?? '') || !/^Ember release run [0-9]+ attempt [0-9]+$/.test(env.DEPLOYMENT_MESSAGE ?? '')) {
      throw new Error('Missing verified deployment attempt identity')
    }
    const current = await currentDeployment(api)
    if (current.id === env.PREVIOUS_DEPLOYMENT) {
      output('deployment', '')
      log('The deployment attempt did not replace the previous installer.')
      return
    }
    if (current.annotations?.['workers/message'] !== env.DEPLOYMENT_MESSAGE) {
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
    if (!uuid.test(env.CANDIDATE_DEPLOYMENT ?? '') || current.id !== env.CANDIDATE_DEPLOYMENT) {
      throw new Error('Current Worker deployment differs from this release attempt; refusing a stale rollback')
    }
    await api({ method: 'POST', body: JSON.stringify({ strategy: 'percentage', versions: [{ version_id: version, percentage: 100 }], annotations: { 'workers/message': `Restore previous installer after failure of deployment ${current.id}` } }) })
    log('Previous installer restored. Website promotion withheld.')
  } else {
    throw new Error('Expected previous, candidate, or restore')
  }
}

async function main() {
  const account = process.env.CLOUDFLARE_ACCOUNT_ID
  const token = process.env.CLOUDFLARE_API_TOKEN
  if (account !== 'b9a05c0456567559ed575566fc5bb3f3' || !token) throw new Error('A scoped Cloudflare release credential is required')
  const endpoint = `https://api.cloudflare.com/client/v4/accounts/${account}/workers/scripts/nyllon-get/deployments`
  const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }
  const api = async options => {
    const response = await fetch(endpoint, { headers, signal: AbortSignal.timeout(30_000), ...options })
    const result = await response.json()
    if (!response.ok || !result.success) throw new Error(`Cloudflare deployment operation failed (HTTP ${response.status})`)
    return result.result
  }
  const output = (key, value) => appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`)
  await manageDeployment(process.argv[2], process.env, api, output)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main()
