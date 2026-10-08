# Matchbox page DOM snapshots

Matchbox owns the application-page design. The `a/` and `b/` directories contain reviewed normalized
DOM tokens rendered from the frozen Rails fixture inputs in the neighboring `a/` and `b/` golden
directories. They are presentation expectations for this fork, not replacement upstream evidence.
Historical Rails HTML, JSON facts, and assets remain unchanged.

`parity_a.rs` selects Matchbox snapshots for application-layout pages and redesigned sidebar
fragments. The view-B tests select them for page renders. Unchanged message, rich-text, and protocol
fragments still compare with the original Rails goldens. Existing behavior assertions remain in the
tests alongside these presentation snapshots.

After making an intentional UI change, rebuild snapshots explicitly:

```sh
MATCHBOX_UPDATE_VIEWS=1 cargo test -p matchbox_views
cargo test -p matchbox_views
```

Review the JSON diff and the affected pages before committing it. The update environment variable
is opt-in; normal test runs fail on a changed or missing snapshot.

The native browser gate (`npm run test:kro --prefix parity`) tests actual form submissions,
message interactions, desktop/mobile layouts, keyboard controls, and light/dark appearance against
fresh temporary app instances. Those checks complement DOM snapshots; neither implies unchanged
Rails pixels. The reference comparison retains strict HTTP-status, network, and Cable checks,
including deliberate mismatches that need a narrow review rather than a blanket exemption.
