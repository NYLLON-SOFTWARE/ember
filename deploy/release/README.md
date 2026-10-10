# Independent stable releases

The public installer remains HTTP 503 until a stable release passes every gate. The checked-in
`deploy/installer/published.mjs` deliberately contains `null`; ordinary source merges do not
activate installation. Only the stable workflow generates published Worker content from the
exact tested artifact bundle. Main images and prereleases never advance the stable installer.

## One-time repository configuration

1. Make the `ember` GHCR package in `NYLLON-SOFTWARE` public through its package settings. Repository
   visibility does not grant anonymous container pulls. Grant this repository Actions access to
   the package. The workflow performs an unauthenticated pull using an empty Docker config.
2. Create the `ember-release` GitHub environment. Store `CLOUDFLARE_API_TOKEN` there, scoped to the
   Kevin Rose account `b9a05c0456567559ed575566fc5bb3f3` with Workers Scripts edit and Account Settings
   read, plus Workers Routes edit and Zone read only for `nyllon.com` if Wrangler needs to reconcile
   the existing custom domain. Do not use a global API key. Keep DNS changes scoped to that zone.
   The `nyllon-get` Worker and `get.nyllon.com` custom domain already exist; workers.dev is disabled.
3. Enable GitHub-hosted `ubuntu-24.04` and `ubuntu-24.04-arm` runners. Test guests use native QEMU
   TCG, so nested virtualization and a separate cloud account are unnecessary. Allow sufficient
   Actions time for four prepublication and four final public guests.
4. Require ordinary CI/review for main. Stable releases are manually dispatched from main and
   select a full commit SHA already on main. Release jobs never bypass branch protections.

## Publish

After merging and reviewing the release implementation, dispatch **Build images and release
Ember** from `main`, selecting a never-used stable `MAJOR.MINOR.PATCH` version and exact main SHA.
The workflow is serialized without cancellation. It rejects reused version tags and versions
older than any previous stable release, including incomplete publications.

The sequence is:

- Rerun all ordinary CI against the selected commit, including seeded Rust tests, frontend,
  benchmark contracts, ShellCheck, installer lifecycle tests, and release identity checks.
- Build native amd64 and arm64 images, record their immutable digests, assemble one candidate
  index from those digests, attest the builds, and sign the index using GitHub OIDC/Cosign.
- Require anonymous registry access and generate deterministic installer assets. `release.json`
  records schema, version, source SHA, image index digest, canonical bundle URL, and SHA-256.
- Test that exact image and bundle in fresh Ubuntu 24.04/Debian 13 guests on both architectures.
- Reject existing container version tags and reserve the Git version tag before promoting the
  same index to immutable version tags and publishing its GitHub release/assets. A failed
  publication keeps that version reserved.
- Capture the previous Cloudflare version and deploy the tested bootstrap, manifest, and guide.
- Download the public bootstrap and bundle anonymously, check exact bytes/checksum, and install
  them in four fresh guests. Only then advance major/minor/latest aliases and the GitHub latest
  release, and emit the website installation link in the workflow summary.

Mutable aliases deliberately advance after final public verification, making the same tested
release the default only once the public distribution path is verified. A failed public test or
failed deployment restores the previous Worker version (HTTP 503 for the first release). Rollback
checks the active deployment identity so an old job retry cannot replace a newer installer; a
previous version already active is a harmless no-op. Version
identities remain reserved; investigate the failure and publish a new version rather than
reusing a tag or overwriting a release asset. Website promotion is withheld.

## Acceptance environment

`deploy/verification/guests.lock.json` pins official Ubuntu and Debian cloud images by immutable
build URL and checksum. Each job creates an isolated runner bridge and a fresh disk. Guests start
without Docker. No live server, production secret, or external cloud resource is used.

A local HTTPS mirror serves the exact final GitHub artifact URLs during candidate verification;
artifact bytes and the bootstrap's embedded checksum remain unchanged. A digest-pinned Pebble
server performs actual HTTP-01/TLS-ALPN challenges against guest ports 80/443. Its test CA and the
mirror CA are trusted explicitly in the disposable guest, application container, browser, and
API client. HTTPS verification is never disabled. Advanced installer `--runtime-env`/`--test-ca`
arguments select this test CA; normal installations use the public ACME service and system trust.
The final public guests fetch the real public installer and GitHub assets without interception.

Guest tests exercise protected browser setup, automatic and ordinary sign-in, messages, upload
and download, WSS subscriptions, native browser push enrollment, installer reruns, reboot with
the test CA offline, complete backups, manual updates, a genuinely failed container startup, and
explicit restore. Synthetic higher-version bundles reuse the exact candidate image to exercise
management transitions and fault injection; they do not claim to test a future schema migration.
Guest preflights and lifecycle tests also cover unsupported systems, insufficient privileges, conflicting ports,
partial downloads, registry failure, archive traversal, operation locking, and interruption.

Diagnostics exclude secrets, cookies, storage snapshots, and raw request traces. A release must
remain unavailable when any required gate cannot run or fails. Passing local checks alone does
not authorize public promotion.

## Limits

This first release supports fresh installations. It does not adopt an existing ONCE installation
or migrate old Rails databases. Keep full backups off-server, encrypted, with matching checksum
metadata. Future database changes need explicit migration/upgrade tests before those versions
can be released. Deployment credentials are operational configuration and are never committed.
