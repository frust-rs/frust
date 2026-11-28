# frust-models

The one place on-device model files live in a Frust app. It lays files out under the app data
directory, keeps a sidecar manifest beside each, and lists, imports and removes them. It does no
inference and knows no model format: runtime plugins and apps ask it for a path and take it from
there.

## Add it

```
frust plugin add models
```

or add the dependency yourself: `frust-models = "0.6"`. It needs no Gradle module or plist key.
On Android it needs the `INTERNET` permission, which a scaffolded app declares only in its debug
and profile manifests. `frust plugin add models` writes
`<uses-permission android:name="android.permission.INTERNET" />` to
`android/app/src/main/AndroidManifest.xml` (the main manifest, so release builds have it) when the
project has an `android/` directory. With the dependency added by hand, add that line yourself, or
every download in a release build fails with a DNS or `Operation not permitted` error.
Features:

- `hub` (default): the Hub client and the downloader.
- `frust-api` (default): the reactive handles described under "In a component". Turn it off
  with `default-features = false` (adding `features = ["hub"]` to keep the downloader) for a build with no `frust-ui` in its graph.

## Platform support

`frust-models` is pure Rust on every target. Its only platform code is the best-effort exclusion
of the store from iOS and macOS device backups and the UI-thread check on Android and Apple. The
registry entry sets no minimum iOS version or Android SDK; on Android a release build needs the
`INTERNET` permission described above. Only the Android leg has been run, on one arm64 phone
with downloads served from a host mirror; the iPhone, macOS and Linux desktop legs have not been
run.

## Quick start

```rust,no_run
use frust_models::{ModelId, ModelStore};

let store = ModelStore::open()?;
let id = ModelId::hub("acme/tiny", "onnx/model.onnx")?;
if let Some(model) = store.get(&id) {
    println!("{} bytes at {}", model.size_bytes, model.path.display());
}
# Ok::<(), frust_models::ModelsError>(())
```

## Where models live

The store root is `<data_dir>/frust-models`, where `data_dir` is `frust_paths::data_dir()`. A
model is identified by `namespace/name/path`; for Hugging Face files that is `author/repo/file`.
The file sits at `<root>/<namespace>/<name>/<path>`, and `path` may nest, for example
`onnx/model.onnx`.

Two sidecars sit next to each file:

- `<file>.part` is a download in progress. `ModelStore::in_progress` reports its size so a
  download can resume. Finishing renames it to the final name.
- `<file>.frust-meta.json` is the manifest: source (Hub repo and revision, URL, or local import), size,
  optional SHA-256, store time, licence and whether the content was verified. A manifest that
  cannot be parsed is logged and treated as absent.

Identifier components may not be empty, start with a dot, end in `.part` or `.frust-meta.json`, or contain
`\`, `:` or control characters, so no identifier can name a sidecar or leave the root.

On iOS and macOS the root and every finished file are excluded from device backups, since models
are large and can be fetched again. This is best effort: a failure is logged and the file still
works.

On Android `frust_paths::data_dir()` is unavailable until the shell has initialised, so
`ModelStore::open` returns `ModelsError::Storage` if it is called earlier. Open the store after
the app has started rather than in a static initialiser.

## Downloading

The default `hub` feature adds a blocking Hugging Face Hub client and a downloader. Build with
`default-features = false` to keep only the store; that build has no HTTP or TLS dependencies.

**Which calls block.** `HubClient::search`, `HubClient::files`, `HubClient::request_for`,
`HubClient::request_for_main` and `DownloadHandle::wait` wait on the network or on a download.
Call them on a worker thread (`frust::spawn_blocking` in an app), never in a component's event
handler or `build`; on Android and Apple (iOS and macOS) a call on the UI thread fails at once with
`ModelsError::UiThread`. `Downloader::start`, `DownloadHandle::cancel`,
`DownloadRequest::from_url` and `HubClient::resolve_url` return without waiting. In a component,
`DownloadTracker::start_hub` (under "In a component") does the blocking part for you.

```rust,no_run
use std::sync::Arc;

use frust_models::{DownloadEvent, DownloadRequest, Downloader, HubClient, ModelId, ModelStore, ModelsError, SearchOptions};

let store = ModelStore::open()?;
let hub = HubClient::new()?.with_token(std::env::var("HF_TOKEN").ok());

// The Hub calls block, so they run on a worker thread rather than the caller's.
let worker = std::thread::spawn(move || -> Result<(), ModelsError> {
    let found = hub.search(&SearchOptions {
        query: Some("tinyllama".into()),
        filter: Some("gguf".into()),
        ..SearchOptions::default()
    })?;
    println!("{} results", found.len());

    // A Hub bundle: sizes and SHA-256 digests come from the repository listing.
    let request = hub.request_for("ggml-org/models", "main", &["tinyllamas/stories260K.gguf"])?;
    let handle = Downloader::start(&hub, &store, request, Arc::new(|event| match event {
        DownloadEvent::Progress { bytes, total, .. } => println!("{bytes} of {total:?}"),
        DownloadEvent::Done(models) => println!("stored {} files", models.len()),
        DownloadEvent::Error(err) => eprintln!("download failed: {err}"),
        _ => {}
    }))?;
    let models = handle.wait()?;
    println!("{} files ready", models.len());
    Ok(())
});

// Any HTTPS URL, with a digest of your own. Building this request does not touch the network.
let id = ModelId::new("mirror", "tiny", "model.onnx")?;
let request = DownloadRequest::from_url("https://example.com/model.onnx", id, Some("ab".repeat(32)), None)?;
# let _ = (worker, request);
# Ok::<(), frust_models::ModelsError>(())
```

- **Search and listings.** `HubClient::search` returns one page of repositories (id, author,
  gating, downloads, likes, last change, file names). `HubClient::files` lists a repository at a
  revision with each file's size and, for files stored in LFS, its SHA-256. `resolve_url` builds
  a file's download URL.
- **Requests.** `HubClient::request_for(repo, revision, paths)` turns paths into a
  `DownloadRequest` and records the Hub repo and revision in each manifest; `request_for_main`
  uses `main`. `DownloadRequest::from_url` downloads any `https://` URL; plain `http://` is
  accepted only for loopback addresses. A request may hold several files, for example a model,
  its `config.json` and its `tokenizer.json`; they download in order as one job.
- **Events.** `Downloader::start` returns at once and runs the job on a `frust-models-dl`
  thread. The sink sees `Started`, then `Progress` at most every 100 ms or 1 MiB with byte
  counts summed over the whole bundle, `Verifying` per file, and exactly one of `Done`,
  `Cancelled` or `Error`. The sink runs on that thread: hand events to the UI and return.
  `DownloadHandle::wait` returns the same outcome; a cancelled job returns
  `ModelsError::Cancelled`; it blocks, so call it off the UI thread. Only one job at a time may
  target a given file (`ModelsError::Busy`).
- **Resume and verification.** A file already stored with the expected SHA-256 is skipped. An
  existing `.part` is hashed and the rest requested with `Range`; a server that ignores the
  range sends the whole file and the download restarts from zero. Each file is hashed while it
  is written, then its size and SHA-256 are checked before it is moved into place. A mismatch
  deletes the part and reports `HubError::SizeMismatch` or `HubError::Integrity`; a cancel or a
  network failure keeps it for the next attempt. When a later file of a bundle fails, the files
  before it stay stored. A manifest is marked `verified` only when the file had a published
  digest to check. Only a download with a SHA-256 (a Hub file stored in LFS, or a `from_url`
  request given one) is tamper-evident; without one, the size is the only check and the bytes
  are only as trustworthy as the TLS connection that carried them.
- **Tokens.** The token is sent as `Authorization: Bearer` only to the Hub origin (scheme, host
  and port of the base URL), on the first request and on any redirect hop that stays on that
  origin, so the CDN that serves the file bytes never sees it. A download whose first URL is not
  on the Hub origin never carries the token, even when a redirect lands on the Hub. A token with
  a plain-`http://` base URL off loopback is refused with `HubError::InsecureUrl` before any
  request.
  `HubClient`'s `Debug` output redacts it, and it is never logged. Gated or private
  repositories answer `HubError::Unauthorized`, with `gated: true` when the terms still need
  accepting on the Hub; a refused token is `HubError::Forbidden`.
- **Rate limits.** HTTP 429 is `HubError::RateLimited` with the server's `Retry-After`, when it
  gives one in seconds. Nothing retries automatically.
- **Size limit.** `DownloadRequest::max_bytes` bounds the bundle's total size. It is checked
  against the declared sizes before any request, against the server's announced length, and
  before every write, so a part never grows past it; the part is then deleted and the error is
  `HubError::TooLarge`.
- **Network.** TLS is rustls on the ring provider with the bundled Mozilla roots on every
  target. Connecting times out after 30 s and a stalled response after 60 s without data; there
  is no overall timeout, so large files can take as long as they need.
- **Redirects.** The client follows redirects itself, at most 5 (a sixth is
  `HubError::TooManyRedirects`), and checks every hop before connecting to it: redirects must
  stay on `https://`, and a hop to plain `http://` is `HubError::InsecureUrl` unless the first URL
  and the hop are both on a loopback address. A listing's next page must be on the base URL's
  scheme, host and port, or the call fails with `HubError::CrossOriginPage`.

## In a component

With the default `frust-api` feature, `frust_models::api` turns the store and the downloader
into signals a component's `build` reads.

- `ModelList::new(store)` holds the stored models. It starts empty; call `refresh()` once (for
  example in `init`). `refresh()` and `remove(id)` do their disk work on `frust::spawn_blocking`
  and publish the new listing from the UI thread, so they need the reactive runtime a running app
  has and must be called on the UI thread. `models()` is a `ReadSignal<Vec<LocalModel>>`.
- `DownloadTracker::new()` runs one download at a time. `state()` is a
  `ReadSignal<DownloadState>` (`Idle`, `Running`, `Verifying`, `Done`, `Cancelled`,
  `Failed(String)`) and `progress()` a `ReadSignal<DownloadProgress>` with `bytes`, `total`,
  `bytes_per_sec`, `file_index` and `files`.
- `start_hub(&client, &store, repo, revision, paths)` is the call for an event handler: the
  state is `Running` at once, the repository listing is fetched on `frust::spawn_blocking`, and
  the download then runs on its own thread. A listing that fails or a missing path ends in
  `Failed`. `start(&client, &store, request)` starts a request you already have, such as a
  `DownloadRequest::from_url` one, and also returns at once. `cancel()` stops the download at
  the next chunk, or before it starts if the listing is still being fetched, and keeps the
  partial files.
- For each download the state is `Running` first, then `Verifying` and `Running` may alternate
  (at most one `Verifying` per file), then exactly one of `Done`, `Cancelled` or `Failed` comes
  last. A `build` may miss a short state between frames; `on_state(observer)` sees every one.
- `DownloadTracker::with_list(list)` links a list: the download thread rescans the store and the
  new listing is written just before the state becomes `Done`, so a component that sees `Done`
  already sees the new models.
- Every signal is written on the UI thread. The download thread and the blocking pool never write
  one: they hand each state, progress value and listing to the UI thread, which writes them in
  order on its next pump of the local task queue; a start call writes the waiting ones, `Running`
  included, before it returns. They never read a signal, never block on the UI and never call
  back into the downloader. `on_state` runs on the UI thread, right after the state is written.
  Call `start_hub`, `start`, `refresh` and `remove` on the UI thread: on another thread they panic
  before changing anything. Dropping the tracker does not cancel the download; call `cancel()`
  first.

`examples/download_component.rs` shows the whole shape in one `Component`: a list of stored
models, a download button, a cancel button and a progress line. It compiles under
`cargo test -p frust-models --examples`.

## Testing

`cargo test -p frust-models` runs everything against an in-process loopback server that stands
in for the Hub and its CDN (`tests/support`); nothing touches the network. Tests that need the
reactive runtime call `frust_reactive::ReactiveRuntime::init` first, which starts the blocking
pool `spawn_blocking` uses, makes the test's thread the UI thread and opens no GPU device; they
pump `ReactiveRuntime::pump_local` while they wait, as a shell does each frame, and check that
every signal write lands on the test's thread. Point a store at a scratch directory with
`ModelStore::with_root`.
