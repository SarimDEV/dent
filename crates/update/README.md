# dent-update

Updates from main's builds ([decision 0077](../../docs/decisions/0077-updates-from-main.md)). It reads `latest.json`, the feed CI publishes for every merge to main, and handles one part of a build: downloading it, checking it against its size and sha256, and unpacking it. The daemon (`daemon/src/updates.rs`) and the app (`desktop/src/updates.rs`) both use it.

A build knows its number among main's builds (`number()`, kept at build time from `DENT_MAIN_BUILD`). A build made anywhere else is 0 and never updates itself.
