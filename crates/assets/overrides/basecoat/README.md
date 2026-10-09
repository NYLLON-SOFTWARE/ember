# Basecoat assets

Run `npm ci --ignore-scripts`, then `npm run build` here after changing the frontend.
`npm run check` verifies committed output without rewriting it; `npm test` exercises
appearance preferences and the Turbo lifecycle. Cargo and Docker consume the three
generated files directly and do not need Node.js or npm.

Edit `src/theme.css` for shared colors, fonts, radii, control dimensions, and component
appearance. It imports Basecoat's default Vega style with only the button, card, input,
and native-select component structures, and scans Askama templates and Rust view helpers
for Tailwind utilities. Add an official `basecoat-css/components/...` import there when
a later page needs a new component. Components use the shared `ui-*` classes with
Basecoat's `input`, `select`, `btn`, and `card` primitives.

`src/theme-init.js` applies the browser's `ember:appearance` preference before paint, falling back to
`matchbox:appearance` and then `campfire:appearance` for existing browsers. New choices use the Ember key.
`src/app.js` bundles the Basecoat runtime and the Turbo lifecycle adapter. Import individual
Basecoat component scripts there only when a migrated page needs them. The first-run page
uses native controls plus a small delegated password-visibility toggle, so it needs no
additional Basecoat component scripts. Setup has no language or appearance selectors; it
follows system colors by default and respects existing saved appearance preferences.

Only `app.css`, `app.js`, and `theme-init.js` are published by the asset pipeline. Sources,
dependencies, build scripts, and manifests stay out of the embedded asset inventory. The
generated bundles retain their third-party licenses. Keep generated output in Git.

Tailwind and its CLI are pinned to 4.3.0; the lockfile includes the compatible 2.6.0
watcher rather than the vulnerable watcher pinned by CLI 4.3.3. No build dependencies
ship in the server binary.

Generated sizes for the first-run delivery, measured on 2026-10-09 with Node 24.18.1
(`gzipSync` defaults; transferred sizes can vary with server compression settings):

| Asset | Minified bytes | Gzip bytes |
| --- | ---: | ---: |
| `app.css` | 156,949 | 16,215 |
| `app.js` | 6,786 | 2,754 |
| `theme-init.js` | 545 | 308 |

The stylesheet retains the official Vega style pack, which Basecoat distributes as one
file, plus only the component structures used by this delivery.

The `Legacy` stylesheet profile retains the original application/editor styles. `Basecoat`
currently loads only `app.css`; preserve the editor's vendored styles explicitly when a
later page migration introduces a rich-text editor. Tracked stylesheet tags reload the
document when Turbo Drive crosses profiles, while frame requests carry their current
profile so they can also promote a cross-profile response to a full navigation.
