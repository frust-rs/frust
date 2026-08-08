# frust-database

A platform-independent, **synchronous, pure-Rust local SQL database** for frust
apps — an in-process SQLite store (the default `engine-sqlite` backend via
`rusqlite`) or the in-process turso store (the optional `engine-turso`
backend, an async engine bridged onto this crate's synchronous API). Unlike
every other plugin under `plugins/`, this crate carries **no** `frust-plugin`
dependency: it needs no JNI/platform handle, since both SQL engines reach
on-disk storage directly through their own FFI/bindings rather than through an
OS capability API. File compatibility is maintained across engines via a
strict interop discipline (WAL journal mode, no MVCC/encryption pragmas).

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

---

## 1. Add the dependency (the only step)

```toml
# app Cargo.toml — [dependencies]
frust-database = { path = "<frust>/plugins/database" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote. That's it: with just this line,
SQLite-backed storage works on every platform.

**No manifest, plist, permission, Gradle module, or Swift package is
needed** — local SQL storage requires no OS permission on any platform, and
both the SQLite and turso backends are plain Rust-to-C FFI (or pure Rust, for
turso) with no Kotlin/Swift glue to wire in. The frust TUI's **Add Plugin**
dialog still lists this plugin (for a consistent workflow across every
plugin), but all it applies is the Cargo dependency line above.

---

## 2. Quick start

Open a database at the standard location (`<data_dir>/databases/<name>.db`),
create a table, insert, and query. Every [`Database`] operation is a
**blocking** synchronous call and must run on a background thread via
`frust_reactive::spawn_blocking`:

```rust
use frust_database::Database;

let db = Database::open("app")?;

let inserted = frust_reactive::spawn_blocking(move || {
    db.execute(
        "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, body TEXT)",
        (),
    )?;
    db.execute("INSERT INTO notes (body) VALUES (?1)", ["hello world"])
})
.await??;

println!("Inserted {inserted} rows");
```

A more complex example with a transaction:

```rust
use frust_database::{Database, Value};

let db = Database::open("app")?;

let result = frust_reactive::spawn_blocking(move || {
    db.transaction(|txn| {
        txn.execute(
            "INSERT INTO notes (body) VALUES (?1)",
            ["first note"],
        )?;
        txn.execute(
            "INSERT INTO notes (body) VALUES (?1)",
            ["second note"],
        )?;
        // Commits on Ok; rolls back on Err
        Ok(())
    })
})
.await??;
```

Query and read results:

```rust
use frust_database::{Database, Value};

let db = Database::open("app")?;

let rows = frust_reactive::spawn_blocking(move || {
    db.query("SELECT id, body FROM notes WHERE id = ?1", [1i64])
})
.await??;

for row in rows {
    if let Some(Value::Text(body)) = row.get(1) {
        println!("Note: {body}");
    }
    // Or by column name:
    if let Some(Value::Text(body)) = row.get_named("body") {
        println!("Note: {body}");
    }
}
```

Every parameter in the examples above — each `["value"]` or `[1i64]` — is a
statement parameter list. The crate accepts arrays and slices of anything
that converts into [`Value`] (the five SQL storage classes: `Null`,
`Integer`, `Real`, `Text`, `Blob`), or an empty tuple `()` for no parameters:

```rust
use frust_database::{Database, Value};

let db = Database::open("app")?;

frust_reactive::spawn_blocking(move || {
    db.execute(
        "INSERT INTO items (id, name, price, data) VALUES (?1, ?2, ?3, ?4)",
        [
            Value::Integer(42),
            Value::Text("item name".into()),
            Value::Real(19.99),
            Value::Blob(vec![1, 2, 3]),
        ],
    )?;

    // Or derive conversions from primitive types
    db.execute(
        "INSERT INTO items (id, name, price) VALUES (?1, ?2, ?3)",
        [42i64, "item", 19.99],
    )?;

    // Empty tuple for no parameters
    db.execute("VACUUM", ())
})
.await??;
```

---

## 3. Never call a database operation on the UI thread

Every [`Database`] operation (`execute`, `query`, `transaction`) is a
**blocking** synchronous call — it runs on the calling thread and blocks until
the SQL operation completes. Calling it directly on the platform UI thread
freezes the frame loop. Pair every database call with `frust_reactive::spawn_blocking`
(an app-tier concern — the plugin itself stays framework-free per the
platform-plugin charter):

```rust
// ✗ Wrong: freezes the UI
let rows = db.query("SELECT * FROM notes", ())?;

// ✓ Right: runs on a background thread
let rows = frust_reactive::spawn_blocking(move || {
    db.query("SELECT * FROM notes", ())
})
.await??;
```

UI-thread discipline is **docs-only here, matching `frust-secure-storage`'s
own precedent for calls it can't cheaply guard in code** — there is no
code-level guard, and adding one would require an FFI dependency this
pure-Rust crate deliberately carries none of.

The `engine-turso` backend additionally reports `DatabaseError::AsyncContext`
rather than panicking if a database call is made directly from inside an
async runtime's `block_on` body (as opposed to from inside a
`spawn_blocking` closure, which is the sanctioned path and is never
rejected) — see §5.2.

---

## 4. Threading model: one serialized connection per handle

A [`Database`] wraps exactly one engine connection behind a `Mutex`, so every
call through one handle is serialized — `Database` is `Send + Sync` and cheap
to share (e.g. behind an `Arc`), but two concurrent calls on the *same*
handle queue rather than run in parallel. `Database` does **not** implement
`Clone` — open a separate handle per connection you want instead.

**Open multiple handles for concurrent readers.** Every real backend opens
its file in **WAL (write-ahead logging) journal mode** (see *Interop
discipline* below), which supports concurrent readers alongside one writer,
but only across separate connections — an app that wants read parallelism
opens more than one `Database` handle onto the same file rather than sharing
one handle across threads expecting internal parallelism:

```rust
use frust_database::Database;

let db1 = Database::open("app")?;
let db2 = Database::open("app")?;  // Same file, different connection

// Spawns two independent background tasks; both can read concurrently.
// Each handle moves into its own closure — Database isn't Clone.
let task1 = frust_reactive::spawn_blocking(move || db1.query("SELECT * FROM notes", ()));
let task2 = frust_reactive::spawn_blocking(move || db2.query("SELECT * FROM items", ()));

let (rows1, rows2) = tokio::join!(task1, task2);
```

---

## 5. Engine selection

This crate supports multiple SQL engine backends, selected at compile time
(via Cargo feature) and optionally overridden at open time (via
[`OpenOptions::engine`]). The default is SQLite.

### 5.1 SQLite (default, `engine-sqlite`)

The `engine-sqlite` feature compiles `rusqlite`'s bundled SQLite — an
in-process, synchronous, on-disk SQL database. It is **always available**
when this crate is in your dependency graph unless you explicitly disable
the default features:

```toml
# Your app's Cargo.toml
frust-database = { path = "<frust>/plugins/database" }  # engine-sqlite enabled by default
```

**What it costs:** ~1.0–1.7 MB (measured in the ship profile — see §6).

**What it buys:**
- Zero external service dependencies — the entire database lives in a file
  on your device
- Immediate synchronous operations on every platform
- Standard SQLite `3` format: readable by the desktop `sqlite3` CLI tool and
  any third-party SQLite library
- Full ACID transaction support
- Platform support: Android, iOS, macOS, Linux, Windows

To use it, just open a database the normal way:

```rust
let db = Database::open("app")?;
```

### 5.2 Turso (optional, `engine-turso`)

The `engine-turso` feature compiles the [`turso`](https://crates.io/crates/turso)
crate (exact-pinned to `=0.7.2`) — a **pure-Rust, SQLite-compatible database
engine that runs in-process against a local file**, not a network service.
This is the embedded `turso` crate, distinct from Turso's separately-branded
hosted-cloud offering — nothing in this backend talks to a network, requires
an account, or requires an API token. A `turso`-backed database is a plain
local file, opened and queried entirely in-process, exactly like the
`engine-sqlite` backend.

Turso's own API is `async`; this crate bridges it onto its synchronous
`EngineConn` seam via a single crate-owned background thread running a
current-thread tokio runtime (see `src/turso.rs`'s module doc for the full
bridge design, including the `DatabaseError::AsyncContext` guard mentioned in
§3).

**What it costs:**
- A significant binary-size delta — the crate's own `Cargo.toml` records
  turso as roughly doubling this crate's binary contribution (~5.7 MB
  measured at the ship profile; see §6 for the README's own tracked figure)
- Pre-1.0 upstream churn — this crate exact-pins the dependency
  (`turso = "=0.7.2"`) rather than allowing a range, precisely because the
  crate hasn't reached a stable 1.0 API yet
- Experimental upstream indexes — see *Caveats* (§7)

**What it buys:**
- A pure-Rust engine with no C FFI in the dependency graph
- The same on-disk file format and interop discipline as the sqlite engine
  (§5.3) — a `turso`-backed file is not distinguishable from a
  `rusqlite`-backed one by any standard SQLite tool
- A seam this crate can extend later toward turso-specific capabilities such
  as vector search or cloud sync — out of scope for v1, and nothing in this
  crate's public API exposes them today, but the engine-selection seam
  (`Engine`/`OpenOptions`) doesn't preclude adding them behind a future
  feature

To use it, enable the feature and pass the engine explicitly:

```toml
# Your app's Cargo.toml
frust-database = { path = "<frust>/plugins/database", features = ["engine-turso"] }
```

```rust
use frust_database::{Database, Engine, OpenOptions};

let db = Database::open_with(
    "app",
    OpenOptions::new().engine(Engine::Turso),
)?;

// ... the rest of your code is identical to SQLite
let rows = frust_reactive::spawn_blocking(move || {
    db.query("SELECT * FROM notes", ())
})
.await??;
```

### 5.3 Cross-engine file compatibility

Both backends follow a strict **interop discipline** to maintain file
compatibility:

- Every file-backed connection ends up in **WAL journal mode** before any
  statement runs — the two backends get there differently. The
  `engine-sqlite` backend *sets* WAL journal mode itself immediately after
  open. The `engine-turso` backend cannot write a rollback-journal file at
  all, so it never issues a `journal_mode` pragma; instead it *asserts* WAL
  by reading `PRAGMA journal_mode` back after open and refusing the
  connection if the file reports anything else.
- No MVCC session extensions (`PRAGMA journal_mode = mvcc`)
- No encryption pragmas (`SQLCipher`, `cipher`, `hexkey`)
- `PRAGMA foreign_keys = ON` is set on every connection by both backends
  (per-connection, never persisted to the file)

This discipline means a `frust-database` file is always a **plain, unencrypted,
standard-SQLite-tool-readable file** — you can open it with the desktop
`sqlite3` CLI, migrate to/from other SQLite libraries, or switch engines at
will. An empty in-memory database uses the same pragmas for consistency, even
though WAL mode doesn't apply to memory.

---

## 6. Size figures

The binary size impact of `frust-database` dependencies at the shipped profile
(optimized release build):

| Engine | Binary impact | Notes |
|--------|---------------|-------|
| SQLite | 1.0–1.7 MB | `rusqlite`'s bundled SQLite; measured locally |
| Turso | **PLACEHOLDER — to be measured** | Not yet measured at this crate's ship profile for this README; `Cargo.toml`'s own dependency comment records a rough ~5.7 MB estimate (roughly double the sqlite backend) pending an exact recorded figure here |

These figures are approximate, varies by target platform, and assume default
link configuration (you may reduce size further by enabling LTO or other
optimizations).

---

## 7. Caveats

- **v1 is basic SQL only.** No prepared-statement caching, no streaming
  cursors, no migrations, no named parameters (positional only, matching this
  crate's v1 scope). Future enhancements (not v1) include named parameters,
  statement cache, streaming query results, and a migration runner.
- **No code-level UI-thread guard.** Like `frust-secure-storage`, blocking-call
  discipline is docs-only. There is no shared guard helper in this codebase,
  and adding one would require an FFI dependency this pure-Rust crate
  deliberately carries none of. Respect the pairing with
  `frust_reactive::spawn_blocking`.
- **Turso indexes are experimental upstream.** This crate's own cross-engine
  conformance suite deliberately excludes `CREATE INDEX` from the shared
  behavioral contract for this reason — the sqlite backend has its own
  sqlite-only index test, but no equivalent guarantee is made for turso. Use
  indexes cautiously in production turso-backed databases until the upstream
  crate marks them stable.
- **`frust create --overwrite` is a non-issue here** — this plugin adds
  nothing to the generated project besides the Cargo dependency line, so
  there is nothing for `--overwrite` to drop.

---

## 8. Testing this crate itself

The cross-engine **conformance suite** (`src/conformance.rs`) exercises the
`engine-sqlite` backend's basic operations (execute, query, transaction) plus
edge cases (empty parameters, NULL values, type conversions) through one
engine-agnostic suite. The `engine-turso` backend has its own dedicated test
module (`src/turso.rs`), covering the same execute/query/transaction/WAL
contract directly, plus cases specific to its async bridge (the
`DatabaseError::AsyncContext` guard, concurrent callers sharing the bridge
thread).

Run the full test suite (default features — `engine-sqlite` only):

```bash
cargo test -p frust-database
```

Run the turso backend's own tests too:

```bash
cargo test -p frust-database --features engine-turso
```
