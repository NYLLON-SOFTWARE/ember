# Frontend overrides

Files in `overrides/` shadow the reference app's assets of the same logical path (the path under
`app/javascript`, `app/assets/*` or a vendored gem's asset directory), so the Rust app can change its
frontend without editing the `reference/` submodule. `build.rs` puts that directory first on the load
path.

| File | Differs from the reference by |
|---|---|
| `zz-matchbox.css` | Shared neutral workspace styling, responsive navigation, forms, chat, and system light/dark colors |
| `matchbox/shell.js` | Turbo-aware mobile drawer, conversation filtering, active navigation, unread activity, and select-on-entry single-line inputs |
| `controllers/channel_order_controller.js` | Staged admin room ordering with Save/Cancel, and personal starred-room ordering by hold-and-drag or keyboard, with save rollback |
| `controllers/icon_picker_controller.js` | Searchable, staged Lucide icon selection, a lazy cached catalog, keyboard navigation, and a native select fallback |
| `controllers/message_actions_controller.js` | Compact message bar and shared reaction tray for hover and inline controls, viewport-aware placement, Escape/focus behavior, and cleanup before Turbo caching |
| `lucide/catalog.json`, `lucide/LICENSE.txt` | Locally generated canonical Lucide SVGs and their license attribution, pinned through the Basecoat frontend build |
| `controllers/svg_preview_controller.js` | Bounded, lazy SVG previews in isolated image contexts; originals stay download-only |
| `controllers/notifications_controller.js` | Permission request within the click, subscription persistence before readiness, and actionable errors |
| `controllers/rooms_list_controller.js` | Preserves the direct-message picker during background refresh, waits for pending frame loads, supports pages without a current room, and synchronizes the current channel icon from sidebar updates |
| `controllers/messages_controller.js` | Rechecks author grouping and day separators at optimistic-message replacement boundaries after rapid sends, without changing message HTML or the wire protocol |
| `controllers/composer_controller.js` | Rejects files over 250 MB on selection, paste, and drop; shows upload percentage, progress, processing, and failure states while preserving the queue and text draft |
| `models/file_uploader.js` | Enforces the 250 MB cap, reports real transfer progress, and rejects HTTP/network failures and sign-in redirects. No `X-CSRF-Token` header: forgery protection is by `Sec-Fetch-Site` |
| `controllers/copy_to_clipboard_controller.js` | A `url` value: a path, copied as an absolute URL against the page, so the cached message markup that carries it doesn't depend on the request's host |
| `lib/autocomplete/base_autocomplete_handler.js` | Asks for JSON (`Accept: application/json`). The reference passes `{ as: "json" }`, a `@rails/request.js` option, to plain `fetch`, gets HTML and never shows the new-ping suggestions |
| `install-edge.svg` | New: a copy of `external/install-edge.svg` where `pwa/_install_instructions` looks for it. Rails can't find it, so Edge gets a 500 on profile and room pages |

Ember branding adds `matchbox-icon.png` (the original artwork under a new logical name),
`controllers/web_share_controller.js` (shared filenames use Ember), and `public/502.html`
(the startup page title). The `public/` directory shadows static pages and is excluded from the
digested asset manifest. The reference submodule remains unchanged.
