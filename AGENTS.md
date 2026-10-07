# Matchbox

Matchbox, a fork of Campfire in Rust. It started as a port that had to be indistinguishable from the Rails app in
`reference/` (a submodule pinned to the SHA it was matched against), and that parity is done: see
`README.md` and `plans/rust-conversion.md`. The app may now diverge from Rails where that makes it
faster or better.

- The Rails app is still the reference for anything not deliberately changed: when in doubt about
  existing behavior, read the Ruby.
- Stay compatible with existing installs unless told otherwise: the SQLite schema, the storage
  layout, and signed/encrypted cookies (so people stay signed in across an upgrade).
- List every deliberate divergence under "Known differences" in `README.md`, and update the tests
  and parity masks it affects.
- Port-owned frontend changes go in `crates/assets/overrides/`, which shadows the reference's assets
  by logical path. Don't edit `reference/`.

## Layout

| Path | Package | What |
|---|---|---|
| `crates/ruby` | `ruby_compat` | Ruby's own string behaviour the other crates share (ERB escaping, `String#to_i`/`#to_f`/`#strip`, `Float#to_s`, `CGI.escape`, `ERB::Util.url_encode`, Active Record's integer binding, Rack's byte ranges), checked against Ruby by `vectors/ruby_core.json`; no dependencies |
| `crates/rails_compat` | `rails_compat` | Rails signing/encryption/serialization contracts and the Rails formats several crates write (`ActiveSupport::JSON`, `Content-Disposition`), verified by `vectors/`, and the app's `Clock` (`Time.current`, and time travel for tests) |
| `crates/kit` | `matchbox_kit` | Axum adapter, `Ctx`, params, cookies, session, forgery protection, flash, formats, responses, gzip, and the front server (TLS, ACME, HTTP/2, response cache) |
| `crates/routes` | `matchbox_routes` | Path helpers mirroring `config/routes.rb` |
| `crates/db` | `matchbox_db` | rusqlite over the existing schema, models, queries, fixtures loader |
| `crates/richtext` | `matchbox_richtext` | Action Text content pipeline: sanitize, attachments, autolink, plain text |
| `crates/storage` | `matchbox_storage` | Active Storage-compatible blobs, disk service, variants (libvips), previews (ffmpeg) |
| `crates/cable` | `matchbox_cable` | Action Cable protocol server, its WebSocket implementation, and in-process pub/sub |
| `crates/assets` | `matchbox_assets` | Propshaft-compatible digesting, importmap, vendored JS/CSS, port-owned overrides |
| `crates/views` | `matchbox_views` | Askama templates (at the ERB file's relative path) and view helpers |
| `crates/matchbox` | `matchbox` (bin) | Controllers, router wiring, channels, jobs, integrations |
| `parity/` | — | Playwright parity harness, screen inventory, reference Docker setup |
| `reference-tools/` | — | Ruby scripts run inside the reference container to produce `vectors/` |
| `bench/` | — | Load generator, benchmark scripts and recorded results |

## Working rules

- Rust comes from mise if it isn't on the PATH: `mise exec rust@1.98.1 -- cargo ...` (the version
  in `Dockerfile`). The `reference/` submodule must be checked out for `crates/assets` to build.
- `cargo test --workspace --exclude html5ever` runs everything. The app's integration tests need the
  seed data (`parity/bin/seed build`, which needs Docker); without it they pass without running,
  with only a note on stderr, so say so when reporting results. `MATCHBOX_REQUIRE_SEED=1` makes a
  missing seed fail them instead.
- `cargo clippy --workspace --exclude html5ever --all-targets` should stay clean. (`html5ever` is a
  vendored copy with one backported fix and two small additions for Gumbo's parse limits, all
  recorded in its `Cargo.toml`, and identical to upstream otherwise.)
- Format with `cargo fmt --all` (`rustfmt.toml`) before committing. Shared benchmark and browser
  gates live in the public `once-campfire-verification` repository; CI pins its revision. The
  vendored html5ever has its own `rustfmt.toml` that turns formatting off, and generated tables are
  marked `#[rustfmt::skip]`.
- CI also runs `cargo shear` for dependencies declared but not used. When it can't see a real use
  (a build script's `#[path]` modules, a self dev-dependency), list the crate under
  `[package.metadata.cargo-shear] ignored` in that manifest, with the reason.
- Put shared dependency versions in the root `[workspace.dependencies]`, and reference them with
  `foo.workspace = true`.
- When matching existing behavior, read the reference's source. When it depends on Rails or gem
  internals, read the gem source inside the reference image
  (`docker run --rm campfire-reference bundle show <gem>`), not docs or memory.
- Write code that reads like the surrounding code: small, clearly named functions, and comments
  only where the behavior is non-obvious. Cite the reference file (`reference/app/...`) when
  matching Rails, and say why when deliberately diverging from it.
- Tests live beside the code. Golden-vector tests read `vectors/*.json`.
- Performance changes need before-and-after measurements. Keep raw results in ignored `tmp/bench/`
  and document the verified current figures and material limitations; do not commit result JSON.

## Reference container

`parity/` builds the reference image as `campfire-reference` from `reference/Dockerfile` and runs it
in production mode with a fixed `SECRET_KEY_BASE` (see `parity/.env.reference`) so that golden
vectors, seeds and screenshots are reproducible. Those keys are for tests only.
