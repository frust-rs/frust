# frust-network-image

A **network image** plugin for frust apps: an image view that loads a URL over HTTPS (reqwest on
rustls with the ring provider and the Mozilla `webpki-roots` trust store on every target) and caches
the decoded result in memory and the fetched bytes on disk. **Status: pin gate only** — this crate
currently holds just the dependency rows that prove the HTTP/TLS stack builds for Android, iOS and
desktop; the view, loader and caches land in a later phase, so there is no public API to use yet.
