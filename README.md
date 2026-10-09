# Ember

[MIT licensed](MIT-LICENSE) · [GitHub](https://github.com/nyllon-software/ember)

Ember is an independently maintained fork of
[Basecamp's Campfire in Rust](https://github.com/basecamp/once-campfire-rust), developed by
[NYLLON-SOFTWARE](https://github.com/nyllon-software). Campfire was created by
[37signals](https://37signals.com); the Rust implementation is based on the original
[ONCE Campfire Rails application](https://github.com/basecamp/once-campfire).

This repository preserves the upstream Git history and the pinned Rails source in `reference/`.
Ember adds its own interface and configuration changes, documented under
[Known differences](#known-differences). It is an independent project, not an official
37signals or Basecamp release.

The existing SQLite database, storage layout and signed/encrypted cookies remain compatible,
so existing installs can upgrade without migrating data or signing everyone out.

One `ember` executable replaces Ruby, Puma, Redis, Resque and Thruster, with libvips and ffmpeg
for media. The Rails frontend ships with a few [port-owned overrides](crates/assets/OVERRIDES.md).
The app includes TLS, HTTP/2, Web Push, bot webhooks, search and Action Cable-compatible WebSockets.

## Running it

Build Ember from this repository, including the pinned upstream submodule:

```sh
git clone --recurse-submodules https://github.com/nyllon-software/ember.git
cd ember
docker build -t ember .
```

Run the resulting image with persistent storage:

```sh
docker run -d -p 80:80 -p 443:443 \
  -e SECRET_KEY_BASE=... -e VAPID_PUBLIC_KEY=... -e VAPID_PRIVATE_KEY=... \
  -e TLS_DOMAIN=chat.example.com \
  -v ember:/rails/storage \
  ember
```

[ONCE](https://github.com/basecamp/once) can also deploy an image you build and publish to your
own registry. The executable is `ember`; `matchbox` remains available as a compatibility alias.
The application crate lives in `crates/matchbox/`. Configuration uses `EMBER_*` names, with
`MATCHBOX_*` and `CAMPFIRE_*` accepted for existing deployments, in that order of precedence.

- `TLS_DOMAIN` enables automatic Let's Encrypt certificates; `DISABLE_SSL` enables plain HTTP.
- `/rails/storage` holds the database, uploads, backups and certificates. Existing installs must
  keep their storage volume (including its existing name) and secrets.
- Web Push needs a valid P-256 VAPID key pair in URL-safe Base64. `VAPID_SUBJECT` sets the contact
  URL; its default is `https://` plus the first `TLS_DOMAIN`, or the project's URL. Keep this pair
  stable across restarts. The room-header bell requests browser permission and saves the subscription
  before showing per-room settings. Embedded browsers without Push support show instructions to
  use a supported browser; iOS/iPadOS users must install the Home Screen app.
- The release version comes from the binary's Cargo package metadata (currently `1.0.0`, displayed
  as `1.0`). `APP_VERSION`, then `GIT_REVISION`, retain their deployment-override precedence.
  The footer and `X-Version` header use the same value; `X-Rev` carries an explicitly configured
  Git revision. Version information is build configuration, not an editable database preference.
- The app listener on `TARGET_PORT` (3000) binds loopback. `TARGET_BIND` overrides this; that listener
  trusts `X-Forwarded-*` from whoever reaches it. Other settings are in
  [`config.rs`](crates/matchbox/src/config.rs).
- The Dockerfile supports amd64 and arm64.

## Performance

These are historical measurements from the upstream Campfire comparison, retained with attribution.
They are not new benchmarks of Ember.

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
EMBER_REQUIRE_SEED=1 cargo test --workspace --exclude html5ever
cargo clippy --workspace --exclude html5ever --all-targets
parity/bin/candidate build
parity/bin/candidate compare
bench/run
```

Seed generation and parity checks need Docker. Tests without the seed skip app integration tests.
For local development, run `cargo run --bin ember -- server` with `SECRET_KEY_BASE` set
(or `SECRET_KEY_BASE_DUMMY=1`). Build an image with `docker build -t ember .`.

The reference harness compares HTML, DOM, accessibility trees, assets, Cable frames and screenshots
against Rails. Ember now owns application-page presentation: reviewed Ember DOM snapshots and
native browser checks cover its redesign. Presentation exceptions do not exempt HTTP status,
network, or Cable checks. The full Rails browser comparison requires a separate review of changed
asset requests, HTML body hashes, and sidebar broadcasts; it is not a claim of visual parity.
See [`parity/SCREENS.md`](parity/SCREENS.md) for the historical coverage and masks,
[`AGENTS.md`](AGENTS.md) for repository layout and working rules,
[`CONTRIBUTING.md`](CONTRIBUTING.md) for contributions, and [`SECURITY.md`](SECURITY.md) for security reports.

### Ember frontend

All pages use compiled Askama templates and the existing form helpers for field names, escaping,
and multipart handling. The workspace interface is styled in
[`zz-matchbox.css`](crates/assets/overrides/zz-matchbox.css), loaded after the original stylesheets
so rich-text editor and interaction styles remain available. Shared navigation,
unread activity, and the mobile drawer live in
[`matchbox/shell.js`](crates/assets/overrides/matchbox/shell.js). Edit these two files for shared
workspace behavior and appearance; page markup remains in `crates/views/templates/`.

The workspace bundle contains 64,090 bytes of CSS (11,778 gzip) and 7,356 bytes of shell JavaScript
(2,357 gzip), measured with gzip level 9. Room ordering, icon selection, and SVG previews use separate Stimulus
controllers, discovered through the digested importmap alongside the inherited controllers.
The compact message-actions controller adds 7,474 bytes (2,041 gzip).
The shared appearance adapter adds 1,981 bytes (824 gzip),
built from `basecoat/src/workspace-appearance.js` with the shared preference helpers.
Assets use the existing digested URLs and compression pipeline. Message fragment recording and
content-addressed response caches retain their existing mechanisms; the message presentation digest
and index ETag version change when introducing SVG markup and compact actions so old HTML is not reused.

Room icons use [Lucide](https://lucide.dev/icons/), pinned to `lucide-static` 1.53.0. The committed
`crates/assets/overrides/lucide/catalog.json` contains 1,869 canonical icons; the build emits a
sorted Rust lookup table. Chat pages embed only their selected icons, with no icon-library request.
The picker fetches the digested catalog on first opening (990,964 bytes; 111,212 gzip), shows 72
results at a time, and shares that cached catalog across Turbo visits. The icon picker controller
is 7,851 bytes (2,656 gzip). Geometric SVG elements and attributes are validated during generation;
uploads and user-provided markup cannot become room icons. ISC and Feather MIT attribution is
included in `crates/assets/overrides/lucide/LICENSE.txt`.

The setup screen keeps its separate Basecoat Vega profile. Its shared controls live in
`crates/views/templates/components/ui.html`; colors, fonts, radii and component overrides live in
`crates/assets/overrides/basecoat/src/theme.css`. The internal `Legacy` asset profile now includes the
Ember workspace override; it does not mean those pages retain the original appearance. Basecoat's
bundle and the workspace styles are never loaded together.

Frontend dependencies are pinned. After editing the frontend sources or adding Tailwind classes,
regenerate the assets and rebuild the Rust app:

```sh
npm ci --prefix crates/assets/overrides/basecoat
npm run build --prefix crates/assets/overrides/basecoat
npm test --prefix crates/assets/overrides/basecoat
cargo build --bin ember
```

Generated CSS, JavaScript, and the Lucide catalog are checked in and embedded with digested URLs, so ordinary Cargo and
Docker builds do not require Node.js. CI runs the frontend build in check mode to detect stale output.
Basecoat build inputs and npm dependencies are excluded from the served asset inventory.
Stylesheets still require rebuilding the executable; they are not loaded from disk at runtime.

The Ember browser checks start temporary app instances and never use the normal storage directory:

```sh
npm ci --prefix parity
npm exec --prefix parity -- playwright install chromium
npm run test:kro --prefix parity
```

Set `EMBER_BIN` to test a different binary. Native browser checks cover setup, authentication,
workspace navigation, conversations, settings, and responsive layouts without Docker. They create
real disposable accounts and messages rather than using the development database. They do not
replace the reference-seeded integration suite.

Reviewed page DOM snapshots live under `crates/views/tests/golden/matchbox/{a,b}` and render the
frozen Rails fixture inputs. To intentionally update them after reviewing a UI change:

```sh
EMBER_UPDATE_VIEWS=1 cargo test -p matchbox_views
cargo test -p matchbox_views
```

Inspect the resulting snapshot diff. Historical Rails goldens remain unchanged; unchanged message,
rich-text, and protocol fragments still compare against them. See the
[snapshot notes](crates/views/tests/golden/matchbox/README.md).

## Known differences

The app keeps the Rails database, storage and current cookie formats compatible. Deliberate
behavior changes and compatibility limits are listed below.

<details>
<summary>Differences from Rails</summary>

- **Ember branding:** application copy, page titles, translations, sharing prompts, installation
  instructions, the web app manifest, and repository documentation use Ember.
  New workspaces default to Ember; an existing workspace named exactly Campfire or Matchbox displays as
  Ember without rewriting its stored name. Other workspace names and message contents stay intact.
  The executable is `ember`, configuration uses `EMBER_*`, and style-profile headers use
  `X-Ember-Style-Profile`. The `matchbox` executable, `MATCHBOX_*` and `CAMPFIRE_*` configuration,
  and legacy style-profile headers remain compatible. Configuration precedence is `EMBER_*`,
  then `MATCHBOX_*`, then `CAMPFIRE_*`; an explicit empty value also takes precedence.
  Rust crate names, internal asset paths, stored settings, and existing browser appearance
  preferences are retained for compatibility.
  The `_campfire_session` cookie, GlobalID namespace, mention MIME type, and upstream asset module
  paths remain compatible so existing sessions, links, and messages work. Upstream source, URLs,
  copyright notices, recorded benchmarks, and Rails golden fixtures retain their original names.
- **Workspace redesign:** application pages use a neutral interface with a left navigation rail,
  a conversation sidebar, a compact room header, left-aligned message threads, an invitation card,
  and a full-width composer. Small screens use a dismissible sidebar drawer and touch-sized controls.
  Home and direct-message navigation use actual room memberships; Activity shows actual unread
  conversations and counts. Search in the rail opens the existing full message search. Ember
  uses “rooms” consistently in labels, settings, favorites, and search. The sidebar starts with
  rooms directly beneath the workspace name. Room creation lives in workspace
  settings, alongside the admin-only room-creation permission control; existing creation permissions
  still apply. The new-room back button returns to workspace settings, including after switching
  between open and private rooms. The appearance control above the profile avatar offers Light,
  Dark, and System, shares onboarding's browser preference, and follows live system changes in
  System mode. The selected palette applies before paint and persists across visits and tabs.
  Opening a profile does not autofocus the name field. Save confirmations show a single checkmark
  centered within the content pane; descriptive notices and errors keep their text.
  Custom CSS saves submit with a full page navigation so the stylesheet reload does not consume
  the confirmation before it is displayed.
  The direct-message picker distinguishes an empty people list from an unsuccessful typed search;
  opening or clearing the search never prompts the user to try another name.
  The star beside a room name saves a per-user favorite; starred rooms
  stay at the top of that user’s sidebar, preserving personal ordering within each group. Search
  uses a subtle focus border and a vertically centered exit button.
  Follow-up message timestamps appear to the right of the message body on hover or keyboard focus,
  with their space reserved to avoid moving the text; the first message keeps its header timestamp.
  Desktop messages use a 64px text inset, 36px avatars with 6px corners, and 15px text with a 22px
  line height. One-line author/message rows are 52px high and continuation rows are 30px high.
  Date pills use 14px bold text and a 28px height; mobile keeps compact gutters and larger tap targets.
  Message options use a compact bar with three quick reactions, reply or attachment actions, copy
  link, and edit. More reactions opens a small tray with the other reactions and custom boost.
  On desktop the native options disclosure sits inside the bar, giving ordinary editable text
  messages a 266×42px toolbar, 16px from the pane edge and 17px above the row. Hovering opens the
  bar across the row's top edge while keeping a continuous hover area.
  Opening the additional reaction tray keeps the bar anchored in place; near the composer the tray
  opens above it. The bar stays reachable while moving onto it and dismisses after leaving. Only one
  bar opens at a time. Clicking Message options keeps it open for interaction, and keyboard and
  touch users can still open it with that control.
  Custom reactions use a spaced form card with a labeled input, the existing 16-character limit,
  and explicit Add reaction / Cancel actions. The bar stays within the conversation on mobile and
  near scroll boundaries, supports keyboard focus and Escape, and closes before Turbo caches the page.
  Without JavaScript, the disclosure
  includes the full reaction tray. Message show/index/create DOM snapshots and the three
  message-rendering fragment parity exceptions cover this deliberate presentation change.
  The design does not fabricate rooms, people, or an Apps section. Sign-in and invitation signup
  use form cards; account/profile screens have persistent labels and visible actions.
  Search shows recent searches once above the results instead of duplicating the links in navigation.
  First-run retains its separate Basecoat card. All other pages inherit the shared workspace styles, including
  room management, search, bots, and user settings. Account custom CSS remains supported.
  The redesign changes templates, shared CSS, and shell JavaScript; the database, sessions, message
  submission, rich-text editor, uploads, notifications, and fragment/cache mechanisms retain their
  existing contracts. Ember DOM snapshots and browser checks replace Rails pixel/DOM equality
  for owned pages. HTTP outcomes and network/Cable behavior are not blanket-allowlisted.
- **Translation controls:** administrators can toggle **Hide translation buttons** in Account
  settings. Hiding is enabled by default for new and existing installs, including sign-in,
  invitations, room forms, profiles, and the welcome card. The choice is stored in the account's
  existing settings JSON and applies to everyone. Changing it reloads the document to clear Turbo's
  snapshots; other open browsers pick up the choice on their next page load.
- **Workspace controls:** entering an editable single-line field selects its existing value so
  typing replaces it. A subsequent click can place the caret normally; multiline editors retain
  their usual editing behavior. Workspace logos and profile avatars use a single large preview with
  an overlaid camera picker and a separate removal action. Uploads still save immediately and offer
  a submit button without JavaScript. The navigation rail shows the uploaded workspace logo, falling
  back to the Ember mark when no logo is set. Immediate account switches save without success flashes; name/logo
  saves have a readable confirmation. Membership badges use the shared control styling.
- **Unsaved room edits:** new/edit room forms prompt with **Unsaved Changes**, **Discard**, and
  **Save** before in-app navigation or same-document browser back/forward. Escape or the close action
  returns to editing. Save validates and persists before continuing to the requested destination;
  failed saves leave the draft intact. Room create/update requests with `Prefer: return=minimal`
  receive a 204 acknowledgment after saving, so removing your own membership can still finish;
  ordinary form submissions retain their redirects. Access-form switches preserve the name, icon, and membership
  drafts. Reverting values removes the prompt. Room forms bypass Turbo snapshots so discarded
  drafts do not return with Back. Tab close, reload, and cross-document history use the browser's
  standard unsaved-changes warning; browsers do not allow styling that warning. Other forms retain
  their existing save behavior.
- **Room ordering:** hold a room for 400 ms and drag to reorder it, or focus it and use
  Alt + Up/Down. Admins set the workspace default; other members can save a personal override.
  **Reset** restores alphabetical defaults for admins, or returns a member to the workspace order.
  Favorites remain personal and sort first. Default changes refresh connected sidebars without
  replacing personal orders; new members inherit the default. The existing account settings JSON
  stores `matchbox_default_room_order` and per-user `matchbox_channel_order`, with no schema migration
  or changes to account cache timestamps. New/unranked rooms follow saved entries alphabetically.
  Submitted rooms must be visible shared memberships. The admin endpoint rechecks the role in the
  write transaction and preserves ranked private rooms the submitting admin cannot see. Broadcasts
  carry no room IDs; each browser fetches only its authorized sidebar.
- **New direct messages:** the sidebar action and rail DMs button open a shared people-search modal.
  Search matches full names; a leading `@` is accepted as a convenience, not a unique username.
  Select one or more people to start or reopen a conversation through the existing direct-room
  endpoint. The modal supports keyboard selection, pagination, mobile layouts, retry after a failed
  search, and retains recipients across background sidebar updates. The original direct-message
  page remains the link destination when the modal enhancement is unavailable.
- **Room icons:** room creators and administrators can choose a Lucide icon when creating or
  editing open or restricted rooms. A searchable, keyboard-accessible picker stages the choice;
  **Use icon** applies it to the form and **Save** persists it for everyone. Cancel keeps the prior
  choice; the default hashtag can be restored. Without JavaScript, the complete catalog is available
  in a native select. Icons appear in the sidebar and room header, including live sidebar updates.
  Canonical names are validated before any other submitted room changes; unknown names return
  422. Choices live in `accounts.settings.matchbox_channel_icons`, requiring no schema migration.
  Type changes retain the icon and room deletion removes it. Only the changed room's timestamp
  is updated; account timestamps and message fragment caches retain their existing behavior.
- **SVG attachments:** SVGs up to 5 MiB gain a lazy image preview. The browser loads the original
  download into an isolated `<img>` data URL; uploaded markup is never inserted into the page DOM.
  Scripts and external resources do not run in the image context. The original remains served as
  `application/octet-stream` with attachment disposition. Oversized or undecodable files retain
  the file card. This is a vector preview, without server-side SVG parsing or a new rendering service.
- **Notification setup:** the bell requests permission within the original click and waits for
  service-worker activation and successful subscription saving before exposing room preferences.
  Denied permission, missing server configuration, unsupported browsers, and retryable failures
  have distinct feedback. A failed new subscription is rolled back and can be retried.
- **Release display:** the former default version `0` is replaced by the actual Cargo release version,
  with the existing deployment overrides retained as described above.
- **Ember setup screen:** `/first_run` uses a responsive Basecoat form card with visible labels and an
  optional camera-style avatar picker, input icons, a password visibility toggle, and a Continue
  button. The setup screen omits the field translation popups and appearance selector. It follows
  system colors by default and honors an existing saved appearance preference. Workspace pages use
  system-driven light and dark colors. Setup now requires a password of at least eight characters
  in both the browser and server; rejected submissions create no account and retain the name and
  email for correction. Existing accounts and sign-in behavior are unaffected. Multipart setup
  submissions and signed sessions retain their existing contracts. Crossing stylesheet
  profiles reloads the document, including a frame request that would embed the new setup form in
  a legacy page. The setup screen has separate KRO visual and behavior checks.
- Session-transfer auto-submit forms explicitly close their form tag; the pinned Rails
  reference omitted it.
- Background sidebar refreshes preserve the legacy New Ping form as well as the new DM picker.
- Rapid message sends recheck author grouping and day separators around replaced optimistic messages.
  A confirmed message no longer keeps its author hidden after the preceding pending message disappears;
  it displays correctly without reloading the conversation.

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
  cookies; Ember uses AES-GCM.

HTTP-01 ACME validation is only unit-tested; TLS-ALPN-01 is tested end to end against a local ACME
server. Rich text is checked against Rails on 658 cases, including 400 fuzzed cases.

</details>

## License and attribution

Ember is distributed under the [MIT License](MIT-LICENSE), the same license as the upstream
Campfire projects. The original 37signals copyright and permission notice are preserved.
Ember contributions are also MIT licensed. Third-party assets and vendored dependencies retain
their own notices, including [Basecoat and Tailwind](crates/assets/overrides/basecoat/LICENSES.txt).

Credit for Campfire, the original Rails application, and the Rust port belongs to their respective
upstream authors and contributors. The Campfire name and original artwork identify that lineage;
this fork is maintained and released as Ember.
