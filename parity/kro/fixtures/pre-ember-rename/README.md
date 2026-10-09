These two scripts are frozen, unmodified copies from Ember commit
`3b12651ea215e7104f8d72b09ba16edcfbf997f8`, immediately before the internal rename:

- `crates/assets/overrides/matchbox/shell.js`
- `crates/assets/overrides/controllers/channel_order_controller.js`

The open-tab upgrade regression loads this previous workspace runtime against initially
legacy-marked HTML, then reconnects its real Cable subscriptions to the current application.
Current sidebar responses and shared-order broadcasts must continue to work without loading
new scripts or navigating the document. Keep these fixtures unchanged so the test continues
to exercise a browser tab left open on the previous release.
