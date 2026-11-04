# Wenlan patch to libsql 0.9.30

This directory contains the published `libsql` 0.9.30 crate. Its generated
`Cargo.toml` declares `license = "MIT"`, but the published crate package does
not include the license text. `LICENSE` is the exact upstream `LICENSE.md` from
the crate's source commit `0653c5788d77ef16a97c56ff3e9fdc11717a72d9`.

Wenlan backports the upstream fix from
[tursodatabase/libsql commit 4e06af572ddf7e3b82a8609c4c623637bae0d778](https://github.com/tursodatabase/libsql/commit/4e06af572ddf7e3b82a8609c4c623637bae0d778).

The local connection wrapper and its inner connection both call `disconnect()`
on drop. Before this fix, each call could pass the same freed SQLite handle to
`sqlite3_close_v2`, causing a use-after-free. `disconnect()` now clears the raw
handle after closing it, so later calls are safe no-ops. No connection, row,
statement, or transaction ownership behavior changes.

Remove this backport when the compatible upstream `libsql` version selected by
Wenlan includes this fix. The lifecycle regression test is
`lint::snapshot::tests::libsql_connection_lifecycle_closes_last_owner_safely`;
run it under Guard Malloc on macOS to detect a double close.

The only functional source change is the upstream close fix. Existing trailing
whitespace in four upstream files is normalized for the repository diff check.
