# Disposable updater fixture

The signed archive contains one non-executable text file:
`Fixture.app/Contents/MacOS/fixture` with `Wenlan updater fixture v2`.
It is signed with a throwaway test key. Only the public key and signature
are retained; these are unrelated to Wenlan's release signing key.

`updater_native_tests.rs` serves it on an ephemeral loopback port and points
the real macOS updater at a temporary fake app. The tests do not launch or
replace Wenlan, access its daemon/data, use production endpoints, or restart.
The same test also rejects a tampered archive and preserves the old fixture.

`install-fault.c` is compiled into a temporary dyld interposer by the macOS
rollback regression test. The parent test process does not load it. A bounded
child test process uses it to reject the updater's atomic app swap with
`ENOTSUP`, then fail the staged-app replacement with `EIO` once. The test
checks that the previous executable remains byte-identical and that a retry
without injected faults installs the signed v2 fixture. The interposer requires
an exact temporary app path and a one-shot marker supplied only to the child;
it has no persistent environment or production fault hooks.
