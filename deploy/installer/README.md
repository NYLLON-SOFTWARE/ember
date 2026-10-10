# Public installer and `emberctl`

The Cloudflare Worker serves a reviewed, version-pinned bootstrap at
`https://get.nyllon.com/ember`. It remains unavailable until the release pipeline
publishes a public image and completes the fresh-host gates. DNS and HTTPS for
`get.nyllon.com` are managed by the Worker's Custom Domain; no origin server is
needed. `workers.dev` and preview URLs are disabled.

## Install a fresh server

Supported hosts are Ubuntu 24.04 and Debian 13 on amd64 or arm64, with systemd,
root access or sudo, and inbound TCP ports 80 and 443 available. Point a hostname's
A record (and any AAAA record) at that server before running:

```sh
curl -fsSL https://get.nyllon.com/ember | sh --
```

The installer asks for the hostname through `/dev/tty`. For an unattended root
session, pass it explicitly:

```sh
curl -fsSL https://get.nyllon.com/ember | sh -s -- --domain chat.example.com
```

The bootstrap downloads one versioned NYLLON release bundle and checks its
embedded SHA256 before extracting or executing it. The complete executable code
is contained in this repository. Python 3, curl, and CA certificates come from
the distribution's signed repositories. A working local Docker installation is
reused; otherwise Docker Engine comes from Docker's official signed apt
repository. Conflicting Docker packages/repository files, occupied ports, and
existing ONCE/Campfire/Ember containers, labelled volumes, or install paths cause
a refusal. Nothing is automatically migrated or removed.

The container runs as UID/GID 1000, publishes ports 80/443, uses the app's ACME
support for HTTPS, and restarts with Docker unless explicitly stopped. The
installer waits for locally connected HTTPS `/up` with normal hostname and
certificate-chain verification. It then prints a private browser setup link.
Create the first administrator using the existing web form. The token is carried
in a URL fragment, exchanged for a short-lived setup cookie, and stops granting
access once the account exists.

The installer generates signing, push, and setup secrets once. Repeating the
installer preserves those secrets and existing data. An installation interrupted
during preparation or before startup can be resumed with its original versioned
bootstrap. Its journal records staging paths before they are created, and a retry
preserves any signing, push, and setup secrets already saved in the private stage.
After the manager is installed, use `sudo emberctl status` and `sudo emberctl
restart` to finish startup after correcting DNS or port problems.

## Operate the server

Run these commands on the server as root or through sudo:

```sh
sudo emberctl status
sudo emberctl logs
sudo emberctl restart
sudo emberctl setup-link
sudo emberctl update
sudo emberctl update 1.0.1
sudo emberctl backup
sudo emberctl restore 20261010T120000Z-abcd1234 --accept-data-loss
sudo emberctl reset-password person@example.com
```

`setup-link` prints the private link only while first setup is pending. `logs`
prints the most recent 200 container log lines. `reset-password` reads a hidden,
confirmed password from a terminal, pipes it privately to the app, revokes the
user's sessions, and restarts the container to terminate existing WebSockets.
Passwords and setup tokens are never passed in process arguments.

Updates are manual. With no version, `update` reads the stable release manifest
at `https://get.nyllon.com/releases/stable.json`; a specified version reads that
version's GitHub release manifest. Only stable semantic versions from the NYLLON
image repository, pinned by SHA256 digest, are accepted. The target image and
matching checked installer bundle are downloaded before stopping the app.
After a complete cold backup, the manager starts the candidate and verifies
HTTPS readiness. A successful update also installs the matching manager code.
There is no scheduled updater or runtime use of `latest`.

A failure before the candidate starts preserves or resumes the old version.
Once starting the candidate has been attempted, it may have migrated the database
or accepted writes. A failed candidate is stopped, with its data, previous image,
full backup, and recovery journal retained. Correct the cause and use `restart`,
or explicitly restore the recorded backup. Restoring discards all chats, uploads,
and configuration changes since that snapshot; the command requires
`--accept-data-loss`. The manager never silently restores data or runs an older
binary against a newer database.

## Files and backups

| Path | Contents |
| --- | --- |
| `/etc/ember/app.env` | Signing, VAPID, and setup secrets; hostname and storage configuration |
| `/etc/ember/state.json` | Install identity, hostname, exact version/source/image digest |
| `/etc/ember/runtime.json` | Explicit advanced ACME options (empty for production defaults) |
| `/etc/ember/manager/` | Matching management code included in full backups |
| `/var/lib/ember/storage/` | Bind-mounted SQLite databases, uploads, and TLS certificate cache |
| `/var/lib/ember/backups/BACKUP_ID/` | `snapshot.tar.gz` and `backup.json` checksums/metadata |
| `/var/lib/ember/recovery.json` | Durable phase/backup details for interrupted operations |
| `/usr/local/bin/emberctl` | Management entry point |
| `/usr/local/lib/ember/emberctl.py` | Python standard-library management implementation |

Configuration and backup directories are root-only (0700), secret/metadata files
are 0600, and the storage root belongs to UID/GID 1000. Full backups live outside
the active storage tree. `backup` stops the app, verifies no other running Docker
container can write that storage, snapshots the complete storage/configuration
and matching manager, verifies SHA256 checksums, then restores the original
running or stopped state. It causes brief downtime. Backups and restarts record
recovery intent before stopping Docker. After an interruption, `status` reports
the operation and `restart` finishes recovery; a backup begun while Ember was
already stopped leaves it stopped. The app's existing `ember backup` command remains the separate
ONCE-compatible SQLite-only snapshot operation.

Restore accepts only a managed backup ID, verifies archive and per-file checksums,
rejects traversal, duplicate paths, links, special files, unexpected ownership,
and incomplete configuration, then stages the full restore before stopping the
app. It restores storage, configuration, and matching management code together.
Superseded directories remain root-private for manual review; backups and previous
images are not automatically pruned. Reserve space for full snapshots and copy
backups off the server using your normal encrypted backup process. A snapshot on
the same disk does not survive loss of that disk.

Management operations use an exclusive advisory file lock and an fsynced recovery
journal. `status` reports any unfinished phase and its backup ID. Do not delete the
journal to force a downgrade; use the explicit recovery commands.

## Release artifacts

Build the bundle once from the exact candidate image index and source commit:

```sh
python3 deploy/installer/build-bundle.py \
  --version 1.0.0 \
  --image-digest sha256:FULL_MULTIARCH_INDEX_DIGEST \
  --source-sha FULL_40_CHARACTER_COMMIT_SHA \
  --output-dir tmp/release-assets
```

Outputs are `ember-installer-VERSION.tar.gz`, `bootstrap.sh`, `release.json`, and
`SHA256SUMS`. Tar/gzip metadata is normalized for reproducible bytes. The bundle
contains `install.sh`, `emberctl`, `emberctl.py`, and `install.json`.

The public `release.json` contract is:

```json
{
  "schema": 1,
  "version": "1.0.0",
  "source_sha": "40 lowercase hex characters",
  "image": "ghcr.io/nyllon-software/ember@sha256:64 lowercase hex characters",
  "bundle": {
    "url": "https://github.com/NYLLON-SOFTWARE/ember/releases/download/v1.0.0/ember-installer-1.0.0.tar.gz",
    "sha256": "64 lowercase hex characters"
  }
}
```

`install.json` contains the same identity without `bundle`, avoiding a circular
checksum. Test these exact artifacts in all four fresh OS/architecture guests,
then promote the already tested image index and publish the release assets.
Deploy the same `bootstrap.sh` bytes through the Worker; the Worker does not
fetch mutable GitHub source during a request. See `deploy/release/` and
`deploy/verification/` for publication and VM gates.

For isolated ACME tests only, the installer accepts `--runtime-env FILE
--test-ca FILE`. Root test sessions may instead export `EMBER_INSTALL_RUNTIME_ENV`
and `EMBER_INSTALL_TEST_CA` with those file paths, so the exact advertised command
can run through a real terminal and prompt for its hostname. Explicit flags take
precedence over these environment defaults; the hostname has no environment
default. These options use the same file validation and never execute the env
file. The env file must contain exactly `ACME_DIRECTORY=https://...`
and `SSL_CERT_FILE=/run/ember-test-ca.pem`. The provided CA bundle is copied into
root-private configuration and mounted read-only into the container; HTTPS
readiness trusts it explicitly. The disposable guest must also trust the local
release mirror's CA through its normal distro trust store. Production defaults
add no CA or alternate directory, and no test path disables certificate checks.

Run the fast installer verification without root, Docker, or network:

```sh
sh -n deploy/installer/install.sh deploy/installer/emberctl
python3 -m unittest discover -s deploy/installer/tests -p 'test_*.py' -v
```

The tests exercise actual bundle/checksum handling, partial-download refusal,
real SQLite/file/configuration restoration, unmanaged-install refusal, locks,
startup interruption, pre-cutover recovery, and preserving new writes after a
failed cutover. The fresh guest gate supplies real Docker, systemd, ACME, browser
setup, reboot, and anonymous registry coverage.

## Worker deployment

Kevin Rose's Cloudflare account hosts `nyllon-get`. Deploy after the release gate
has selected the published artifacts:

```sh
npx wrangler deploy --config deploy/installer/wrangler.jsonc
curl -fsS https://get.nyllon.com/releases/stable.json
curl -fsS https://get.nyllon.com/ember
```

Before release, `/ember` returns HTTP 503 with a harmless shell message and exits
nonzero. A shell pipeline can hide curl's own exit status unless the caller uses
`pipefail`; release verification checks the HTTP status and script bytes directly.
