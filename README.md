# Matchbox

[MIT licensed](MIT-LICENSE) · [GitHub](https://github.com/nyllon-software/matchbox)

Matchbox is an independently maintained fork of
[Basecamp's Campfire in Rust](https://github.com/basecamp/once-campfire-rust), developed by
[NYLLON-SOFTWARE](https://github.com/nyllon-software). Campfire was created by
[37signals](https://37signals.com); the Rust implementation is based on the original
[ONCE Campfire Rails application](https://github.com/basecamp/once-campfire).

This repository preserves the upstream Git history and the pinned Rails source in `reference/`.
Matchbox adds its own interface and configuration changes, documented under
[Known differences](#known-differences). It is an independent project, not an official
37signals or Basecamp release.

The existing SQLite database, storage layout and signed/encrypted cookies remain compatible,
so existing installs can upgrade without migrating data or signing everyone out.

One `matchbox` executable replaces Ruby, Puma, Redis, Resque and Thruster, with libvips and ffmpeg
for media. The Rails frontend ships with a few [port-owned overrides](crates/assets/OVERRIDES.md).
The app includes TLS, HTTP/2, Web Push, bot webhooks, search and Action Cable-compatible WebSockets.

## Running it

Build Matchbox from this repository, including the pinned upstream submodule:

```sh
git clone --recurse-submodules https://github.com/nyllon-software/matchbox.git
cd matchbox
docker build -t matchbox .
```

Run the resulting image with persistent storage:

```sh
docker run -d -p 80:80 -p 443:443 \
  -e SECRET_KEY_BASE=... -e VAPID_PUBLIC_KEY=... -e VAPID_PRIVATE_KEY=... \
  -e TLS_DOMAIN=chat.example.com \
  -v matchbox:/rails/storage \
  matchbox
```

[ONCE](https://github.com/basecamp/once) can also deploy an image you build and publish to your
own registry. The executable is `matchbox` and the application crate lives in `crates/matchbox/`.

- `TLS_DOMAIN` enables automatic Let's Encrypt certificates; `DISABLE_SSL` enables plain HTTP.
- `/rails/storage` holds the database, uploads, backups and certificates. Existing installs must
  keep their storage and secrets.
- Web Push needs a valid P-256 VAPID key pair in URL-safe Base64. `VAPID_SUBJECT` sets the contact
  URL; its default is `https://` plus the first `TLS_DOMAIN`, or the project's URL.
- The app listener on `TARGET_PORT` (3000) binds loopback. `TARGET_BIND` overrides this; that listener
  trusts `X-Forwarded-*` from whoever reaches it. Other settings are in
  [`config.rs`](crates/matchbox/src/config.rs).
- The Dockerfile supports amd64 and arm64.

## Performance

These are historical measurements from the upstream Campfire comparison, retained with attribution.
They are not new benchmarks of Matchbox.

Measured with 16 concurrent clients on an AMD Ryzen AI MAX+ 395 with 32 GB RAM,
with four hardware cores allocated to each app.

| HTTP workload (requests/sec) | Rails | [Django](https://github.com/basecamp/once-campfire-django) | [Laravel](https://github.com/basecamp/once-campfire-laravel) | [Express](https://github.com/basecamp/once-campfire-express) | [Elixir](https://github.com/basecamp/once-campfire-elixir) | [Go](https://github.com/basecamp/once-campfire-go) | [Rust](https://github.com/basecamp/once-campfire-rust) | [C](https://github.com/basecamp/once-campfire-c) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Room page | 230 | 62 | 760 | 2,622 | 942 | 31,673 | 35,484 | 141,834 |
| Messages page | 402 | 70 | 924 | 3,245 | 1,267 | 30,746 | 40,674 | 151,564 |
| Sidebar | 468 | 229 | 1,383 | 34,938 | 2,515 | 18,586 | 34,479 | 159,850 |
| Search | 399 | 118 | 1,135 | 6,613 | 1,814 | 29,765 | 34,432 | 155,456 |
| Post a message | 248 | 112 | 498 | 2,088 | 1,400 | 9,073 | 8,998 | 7,460 |

[Shared verification](https://github.com/basecamp/once-campfire-verification) · [Detailed results](https://github.com/basecamp/once-campfire-verification/blob/main/docs/performance-review.md).

Database scheduling, rich text rendering and cached-page gzip improvements contributed by
Daniel Collin ([emoon](https://github.com/emoon)) in
[#43](https://github.com/basecamp/once-campfire-rust/pull/43).

## Development

Check out the `reference/` submodule before building. Rust 1.98.1 is available through mise;
native media dependencies are specified in the [`Dockerfile`](Dockerfile).

```sh
git submodule update --init
parity/bin/reference build
parity/bin/seed build
MATCHBOX_REQUIRE_SEED=1 cargo test --workspace --exclude html5ever
cargo clippy --workspace --exclude html5ever --all-targets
parity/bin/candidate build
parity/bin/candidate compare
bench/run
```

Seed generation and parity checks need Docker. Tests without the seed skip app integration tests.
For local development, run `cargo run -p matchbox -- server` with `SECRET_KEY_BASE` set
(or `SECRET_KEY_BASE_DUMMY=1`). Build an image with `docker build -t matchbox .`.

The parity harness compares HTML, DOM, accessibility trees, assets, Cable frames and screenshots
against Rails. See [`parity/SCREENS.md`](parity/SCREENS.md) for coverage and masks,
[`AGENTS.md`](AGENTS.md) for repository layout and working rules,
[`CONTRIBUTING.md`](CONTRIBUTING.md) for contributions, and [`SECURITY.md`](SECURITY.md) for security reports.

### Matchbox frontend

The setup screen uses Basecoat's Vega components with compiled Askama templates. Shared controls
live in `crates/views/templates/components/ui.html`; colors, fonts, radii and component overrides
live in `crates/assets/overrides/basecoat/src/theme.css`. The form helpers retain the existing field
names, escaping and multipart handling. Other pages keep their original stylesheet profile until
they are migrated.

Frontend dependencies are pinned. After editing the frontend sources or adding Tailwind classes,
regenerate the assets and rebuild the Rust app:

```sh
npm ci --prefix crates/assets/overrides/basecoat
npm run build --prefix crates/assets/overrides/basecoat
npm test --prefix crates/assets/overrides/basecoat
cargo build -p matchbox
```

Generated CSS and JavaScript are checked in and embedded with digested URLs, so ordinary Cargo and
Docker builds do not require Node.js. CI runs the frontend build in check mode to detect stale output.
Source files, build tools and npm dependencies are excluded from the served asset inventory.
Stylesheets still require rebuilding the executable; they are not loaded from disk at runtime.

The Matchbox browser checks start temporary app instances and never use the normal storage directory:

```sh
npm ci --prefix parity
npm exec --prefix parity -- playwright install chromium
npm run test:kro --prefix parity
```

Set `MATCHBOX_BIN` to test a different binary. The native browser checks cover setup and appearance
without Docker; they do not replace the reference-seeded integration and parity suites.

Future page batches are sign-in/invitation signup, account/profile/admin, room management/search,
then chat/sidebar/composer. Keep each batch on the shared controls and explicitly choose its asset
profile; do not mix the legacy and Basecoat component styles in the same document.

## Known differences

The app keeps the Rails database, storage and current cookie formats compatible. Deliberate
behavior changes and compatibility limits are listed below.

<details>
<summary>Differences from Rails</summary>

- **Matchbox branding:** application copy, page titles, translations, sharing prompts, installation
  instructions, the web app manifest, executable, Rust crates, and developer tooling use Matchbox.
  New workspaces default to Matchbox; an existing workspace named exactly Campfire displays as
  Matchbox without rewriting its stored name. Other workspace names and message contents stay intact.
  `MATCHBOX_*` environment variables replace the old prefix, with `CAMPFIRE_*` accepted as a fallback
  and the new name taking precedence. Existing browser appearance preferences are also honored.
  The `_campfire_session` cookie, GlobalID namespace, mention MIME type, and upstream asset module
  paths remain compatible so existing sessions, links, and messages work. Upstream source, URLs,
  copyright notices, recorded benchmarks, and Rails golden fixtures retain their original names.
- **Translation controls:** administrators can toggle **Hide translation buttons** in Account
  settings. Hiding is enabled by default for new and existing installs, including sign-in,
  invitations, room forms, profiles, and the welcome card. The choice is stored in the account's
  existing settings JSON and applies to everyone. Changing it reloads the document to clear Turbo's
  snapshots; other open browsers pick up the choice on their next page load.
- **Matchbox setup screen:** `/first_run` uses a responsive Basecoat form card with visible labels and an
  optional camera-style avatar picker, input icons, a password visibility toggle, and a Continue
  button. The setup screen omits the field translation popups and appearance selector. It follows
  system colors by default and honors an existing saved appearance preference. Legacy pages retain
  their existing system-driven colors. Setup now requires a password of at least eight characters
  in both the browser and server; rejected submissions create no account and retain the name and
  email for correction. Existing accounts and sign-in behavior are unaffected. Multipart setup
  submissions and signed sessions retain their existing contracts. Crossing stylesheet
  profiles reloads the document, including a frame request that would embed the new setup form in
  a legacy page. The setup screen has separate KRO visual and behavior checks.
- Session-transfer auto-submit forms explicitly close their form tag; the pinned Rails
  reference omitted it.
- Background sidebar refreshes preserve an open New Ping form and selected recipients.

- Sidebar connection refresh waits for the current Turbo frame to finish loading,
  preventing an aborted response on startup or reconnect. Obsolete connections and removed frames do not reload.

- **WebSockets:** `permessage-deflate` without context takeover compresses each broadcast once
  for all subscribers. Decoded messages remain identical.
- **CSRF:** `Sec-Fetch-Site` replaces tokens. Writes accept `same-origin` and `same-site`, reject
  `cross-site` and missing headers over HTTPS with 422, and retain the `Origin` check. Plain HTTP
  accepts missing headers with `SameSite=Lax` cookies. Pages omit CSRF tags and fields; old tabs
  still work, but HTTPS forms require a browser that sends the header (Safari 16.4 or newer).
- **Jobs:** Redis and Resque are replaced by in-process queues with `JOB_CONCURRENCY` workers per
  job kind. Queued pushes and webhooks are lost on a crash; slow webhooks don't block pushes.
- **Push:** invalid VAPID keys disable push at boot. Subscriptions survive TLS/configuration
  failures and are deleted only on 404/410 or an invalid subscription P-256 key. Notification
  bodies are truncated with an ellipsis at 3 KB and titles at 256 bytes. `VAPID_SUBJECT` is configurable.
  Delivery timeouts are 10 seconds per connect/read and 30 seconds overall.
- **Cookies:** sessions are written only on change and deleted when empty; `last_room` only on
  change. `session_token` is re-signed on the hourly activity refresh, retaining its rolling
  20-year expiry. Other authenticated reads avoid the database writer.
- **Caching:** room, messages and search ETags hash cached page parts rather than the body.
  Copy-link buttons cache paths and resolve them against the page URL; bot JSON is cached per
  base URL, preventing a request's Host from changing other users' links.
- **SQLite:** boot adds `index_messages_on_room_id_and_created_at` and
  `index_messages_on_room_id_and_updated_at` if missing. They remain compatible with Rails.
  Memory mapping is disabled; reads use SQLite's page cache.
- **Media formats:** libvips 8.16.1 and ffmpeg 7.1.5 use the Rails image's Debian sources, with
  byte-identical thumbnails, posters and metadata for supported formats. libvips omits loaders
  Rails already blocks. ffmpeg omits external-library-only formats: tracker modules, game-console
  music, JPEG XL/SVG frames, codec2, teletext and DASH/IMF. Tracker/game-console uploads lack
  duration and bit rate. Unused encoders, muxers, hardware and network support are omitted.
- **Media processing:** message uploads are copied and checksummed before saving their rows, then
  deleted if saving fails. The redundant MD5 reread is skipped; analysis, variants, posters and
  client direct-upload checksums still validate files. At most four media jobs run off the database
  writer. Variants/posters are saved already analyzed; concurrent transforms keep the first saved
  result and delete duplicates. ffmpeg posters time out at 60 seconds, ffprobe at 30.
- **Request limits:** non-file-upload bodies and direct uploads are capped at 16 MiB (413).
  Nonnumeric direct-upload sizes and oversized QR codes return 422. Page numbers cap at a billion.
- **Cable limits:** 64 subscriptions per connection, 4 KiB identifiers and 1 MiB messages.
  Clients that don't read for 30 seconds disconnect. Banning/deactivating a user closes their
  connections after commit.
- **Unfurling:** 10 seconds overall, 5 per connect/read, at most 16 concurrent unfurls, and only
  the first 256 attributes of a `meta` tag are read. Timed-out pages unfurl nothing.
- **Webhooks:** 60 seconds overall, 7 per connect/read. Replies over 100 MB after decompression
  fail delivery without posting a response.
- **Front server:** `TARGET_PORT` binds loopback and enforces front-server timeouts and
  `MAX_REQUEST_BODY`. Cache keys count toward `CACHE_SIZE`, preserve raw paths/queries, skip URIs
  over 2 KB and forward range requests. Idle HTTP/1 connections close at the shorter of
  `HTTP_IDLE_TIMEOUT` and `HTTP_READ_TIMEOUT` until request headers arrive (30 seconds with defaults,
  60 with image settings); HTTP/2 uses the idle timeout. Response header lines containing DEL are omitted.
- **Passwords:** bcrypt runs outside database connections/transactions. Unknown emails still
  perform one bcrypt check.
- **JSON:** floats use the shortest equivalent digits. The web app manifest properly JSON-escapes
  account names and URLs.
- **Search:** words are literal full-text terms, including `NOT`, `AND`, `OR` and `NEAR`.
  The newest 100 matches are selected by message id, as in current Rails. Imported messages
  with creation times out of id order follow id order in search. A bounded global scan
  falls back to a membership-scoped query when most recent matches are inaccessible.
- **Routes and UI:** `/rooms/directs/:id` redirects to the room; infinite `Accept` q-values sort
  first or last by sign; EdgeHTML install instructions include the missing image; the new-ping
  picker requests JSON so suggestions appear.
- **Rich text attributes:** autolinking escapes `<`/`>` in attributes to prevent stored XSS.
  Sanitization drops `name` attributes to prevent DOM clobbering. Styles retain only `color` and
  `background-color` with plain keyword/hex/RGB/HSL values or CSS variables in bot/webhook HTML;
  message pages drop styles.
- **Rich text attachments:** content attachments nest at most eight levels; deeper content is
  empty. Deleted-user mentions render ☒ and are omitted in the editor. Active Storage attachments
  embedded in message bodies, which the composer can't create, render ☒.
- **Malformed rich text:** plain-text extraction failures are logged and use empty text or an
  attachment filename; messages are still indexed, pushed, broadcast and sent to bots. Bodies
  beyond 400 nesting levels or 400 attributes per element are stored unchanged with empty plain
  text. These messages render as unrenderable.
- **Not ported:** Active Storage streaming's duplicate `session_token` cookie or legacy AES-CBC
  cookies; Matchbox uses AES-GCM.

HTTP-01 ACME validation is only unit-tested; TLS-ALPN-01 is tested end to end against a local ACME
server. Rich text is checked against Rails on 658 cases, including 400 fuzzed cases.

</details>

## License and attribution

Matchbox is distributed under the [MIT License](MIT-LICENSE), the same license as the upstream
Campfire projects. The original 37signals copyright and permission notice are preserved.
Matchbox contributions are also MIT licensed. Third-party assets and vendored dependencies retain
their own notices, including [Basecoat and Tailwind](crates/assets/overrides/basecoat/LICENSES.txt).

Credit for Campfire, the original Rails application, and the Rust port belongs to their respective
upstream authors and contributors. The Campfire name and original artwork identify that lineage;
this fork is maintained and released as Matchbox.
