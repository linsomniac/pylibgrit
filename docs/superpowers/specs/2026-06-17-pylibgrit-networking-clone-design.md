# pylibgrit Phase C — Networking & Clone (read-path) — Design

**Status:** Approved (2026-06-17)
**Depends on:** Phase A (write-core, 0.2.0) + Phase B (worktree & merge).
**Roadmap:** `docs/superpowers/specs/2026-06-14-pylibgrit-write-core-design.md` §8 (C = networking & clone);
this spec narrows C to a **read-path** milestone and splits push into a future **Phase D**.

---

## 1. Goal & scope

Add a read-only network surface over grit-lib 0.4.1: list remote refs, fetch objects/refs into a
local repository, and clone. grit-lib has **no clone porcelain** — the binding assembles
`init + fetch + checkout` itself.

**In scope (read-path):** `ls_remote`, `fetch`, `clone`; transports **git://** and **https**
(`http-ureq` bundled by default); credentials (explicit + URL userinfo + git credential helpers);
an optional Python progress sink.

**Deferred (explicit non-goals):**
- **push / `git-receive-pack`** → Phase D (receive-pack, pack-building, force/lease, atomic).
- **ssh transport** (spawns the system `ssh`; hard to test hermetically).
- **bare / mirror clone** (`git clone --bare`/`--mirror`) — v1 clones are worktree-only; the
  divergent ref layout (heads into `refs/heads/*`, no `refs/remotes/origin/*`) is a follow-up.
- **shallow / depth / deepen / unshallow** (full clone & fetch only in v1).
- `insteadOf` / url-rewrite, submodule fetch (`.gitmodules`), promisor/partial clone.

### Decisions (locked during brainstorming)

| Topic | Decision |
| --- | --- |
| Phase scope | Read-path (`ls_remote` + `fetch` + `clone`); push is a separate Phase D. |
| Transports | **git://** + **https**; ssh deferred. |
| Packaging | `http-ureq` **bundled by default** (ureq + rustls, statically linked). |
| Auth | explicit `username`/`password` **and** URL userinfo **and** git credential helpers. |
| Progress | optional **duck-typed sink object** (mirrors grit's `ProgressSink`), in v1. |
| Shallow | deferred. |
| Clone fidelity | worktree clone only; write git-faithful `[remote "origin"]` config + `refs/remotes/origin/*` tracking refs. Bare/mirror clone deferred. |
| `ls_remote` impl | built from the **v0/v1 ref advertisement** (`Connection::advertised_refs`), not grit's local-only `ls_remote`. |
| https tests | hermetic via `git http-backend` (skip-if-unavailable). |

---

## 2. Public Python API

```python
# Module-level — needs no local repository.
pylibgrit.ls_remote(
    url: str, *,
    username: str | None = None,
    password: str | None = None,
    use_credential_helpers: bool = True,
    heads: bool = False,
    tags: bool = False,
) -> list[RemoteRef]

# Classmethod, mirrors Repository.init(path, ...). Worktree clone only (bare deferred).
Repository.clone(
    url: str, path,                 # path: str | os.PathLike
    *, branch: str | None = None,   # ref to check out; default = remote HEAD
    username: str | None = None,
    password: str | None = None,
    use_credential_helpers: bool = True,
    progress: object | None = None,
) -> Repository

# Instance method on an already-open repository.
repo.fetch(
    url: str,
    refspecs: list[str] | None = None,   # default ["+refs/heads/*:refs/remotes/origin/*"]
    *, tags: str = "following",          # "none" | "following" | "all"
    prune: bool = False,
    username: str | None = None,
    password: str | None = None,
    use_credential_helpers: bool = True,
    progress: object | None = None,
) -> FetchReport
```

### Value objects (read-only pyclasses)

- **`RemoteRef`** — `name: str`, `oid: str` (hex), `symref_target: str | None`
  (set only for symbolic refs such as `HEAD` when advertised).
- **`RefUpdate`** — `remote_ref: str`, `local_ref: str | None`, `old_oid: str | None`,
  `new_oid: str | None`, `mode: str`, `note: str | None`.
  `mode` is the lower-kebab name of grit's `UpdateMode`
  (`"new" | "fast-forward" | "forced" | "up-to-date" | "rejected" | "tag-update" | …`);
  the exact set is enumerated from grit's `UpdateMode` during planning and surfaced verbatim.
- **`FetchReport`** — `updates: list[RefUpdate]`, `default_branch: str | None`.
  (Shallow fields `new_shallow`/`new_unshallow` are intentionally omitted — shallow is deferred.)

### Types & encodings

- **Ref names / symref targets:** `str` (UTF-8), matching grit's `String` ref model and the
  existing binding's ref methods.
- **OIDs:** lower-hex `str` (40 chars SHA-1 / 64 chars SHA-256), matching Phase A/B getters.
- **`tags`** string maps to `grit_lib::transfer::TagMode` (`none|following|all`).

### Progress sink

`progress=` accepts an optional object **duck-typed** against grit's `ProgressSink`. The binding
calls whichever of these the object defines (missing methods are skipped, never an error):

```python
class ProgressSink(Protocol):           # documentation only; not enforced
    def start(self, label: str, total: int | None) -> None: ...
    def inc(self, units: int) -> None: ...
    def set(self, current: int) -> None: ...
    def message(self, msg: str) -> None: ...
    def finish(self) -> None: ...
```

`None` → grit's `NullProgress` (no calls). Chosen over a single flat callback because it is a
1:1 bridge to the Rust trait and keeps phase/label/total/message structure intact.

---

## 3. Architecture

A thin Rust transport layer dispatches on the URL scheme; each scheme uses the grit entry point
designed for it, and both unify into one `FetchReport`.

### 3.1 Scheme dispatch (`src/net_transport.rs`)

- **`git://…`** → `grit_lib::transport::GitDaemonTransport::new()`.
  - `ls_remote`: `.connect(url, Service::UploadPack, ConnectOptions{ protocol_version: 1, .. })`,
    then read `conn.advertised_refs()` + `conn.head_symref()`. No objects transferred.
  - `fetch`: `.connect(url, UploadPack, ..)` → `grit_lib::fetch::fetch_remote(git_dir, &mut conn, &opts, progress)`.
    **`fetch_remote` does not apply ref updates** — the binding writes each returned
    `RefUpdate` whose mode is `New | FastForward | Forced` via the ref writer (matching what
    `http_fetch` does internally), then optionally writes `FETCH_HEAD` (see §8).
- **`https://…` / `http://…`** → build a `UreqHttpClient` (with credentials, §4).
  - `ls_remote`: `grit_lib::transport::http::SmartHttpTransport::new(client).connect(url, UploadPack, v1)`
    → read the advertisement as above.
  - `fetch`: `grit_lib::transport::http::http_fetch(&client, git_dir, url, &opts, progress)`
    — **self-applies** refs and unpacks objects; the binding maps its `FetchOutcome` to `FetchReport`.
- **Unknown scheme** → `NetworkError` ("unsupported transport: <scheme>; supported: git, http, https").

A scheme `enum` keeps the two code paths behind one `fn fetch(...)` / `fn ls_remote(...)` so the
porcelain in `remote.rs` is transport-agnostic.

### 3.2 Porcelain (`src/remote.rs`)

Houses the three Python entry points and the `RemoteRef` / `RefUpdate` / `FetchReport` pyclasses.

- **`ls_remote(url, …)`** → connect (v1), build `RemoteRef`s from advertised refs (+ HEAD symref),
  apply `heads`/`tags` filters in the binding.
- **`repo.fetch(url, refspecs, …)`** → build `FetchOptions` (default refspec when `None`), dispatch
  by scheme, ensure refs written, return `FetchReport`.
- **`Repository.clone(url, path, …)`** composes existing primitives (worktree clone):
  1. `init(path)` (Phase A; non-bare worktree repo).
  2. Write `[remote "origin"]` config: `remote.origin.url = <url>`,
     `remote.origin.fetch = +refs/heads/*:refs/remotes/origin/*` (§4 config writer).
  3. `fetch(url, default refspec)` → tracking refs `refs/remotes/origin/*` + objects.
  4. Resolve the branch to check out: explicit `branch=`, else `FetchReport.default_branch`,
     else the remote `HEAD` symref; map to `refs/remotes/origin/<name>`.
  5. Create local `refs/heads/<name>` = that oid; set `HEAD` symbolic-ref to `refs/heads/<name>`.
  6. `checkout_tree(commit.tree, update_index=True)` (Phase B overlay — the worktree is empty,
     so overlay == a full checkout).
  7. Return an opened `Repository` for `path`.

### 3.3 New Rust modules

| File | Responsibility |
| --- | --- |
| `src/net_transport.rs` | scheme parse/dispatch; construct `Box<dyn Connection>` (git://) or `UreqHttpClient` (https); shared `FetchOptions` builder. |
| `src/remote.rs` | `ls_remote`/`fetch`/`clone` entry points; `RemoteRef`/`RefUpdate`/`FetchReport` pyclasses; ref-update application for the git:// path. |
| `src/net_progress.rs` | `PyProgressSink`: wraps an optional `Py<PyAny>`, implements grit's progress trait by calling Python methods if present (GIL-aware; see §6). |
| `src/net_credentials.rs` | `StaticCredentialProvider` (explicit/userinfo creds) chained to `grit_lib::credentials::HelperCredentialProvider`. |

`src/lib.rs` registers `Repository.clone`/`fetch`, the module-level `ls_remote`, the three value
classes, and the two new exceptions. `src/repository.rs` gains `clone` (classmethod) and `fetch`.

---

## 4. Credentials

A single binding `CredentialProvider` resolves in this order, the first complete credential winning:

1. Explicit `username`/`password` kwargs.
2. URL userinfo (`https://user:token@host/…`) — **parsed by the binding** and stripped from the
   URL before connecting, because `UreqHttpClient` does **not** honor userinfo itself.
3. If `use_credential_helpers` (default `True`): `HelperCredentialProvider::new(config)` reading
   `credential.helper` / `credential.<url>.helper` from the repo's cascaded `ConfigSet`
   (global/system for `ls_remote` and during `clone`, since the repo exists by fetch time).

Implementation: `StaticCredentialProvider { username, password }` whose `fill()` returns the static
credential when present, else delegates to the wrapped helper (or returns the input unchanged when
helpers are disabled). Wired onto the client via `UreqHttpClient::with_credential_provider(Box<…>)`.
The provider is consulted by https **only on HTTP 401**; git:// and unauthenticated https never
invoke it. Helper `approve`/`reject` side effects (e.g. `store` caching a token on success) are
git-faithful and intentional (§8).

---

## 5. Error handling

Extend `src/error.rs`:

- Add `AuthenticationError` and `NetworkError` as subclasses of the existing base `GritError`
  (alongside `RepositoryError` etc.); register both in `register()`.
- Route `grit_lib::error::Error::Auth(_)` → `AuthenticationError` in `map_err` (it currently falls
  through to the `GritError` catch-all; `Auth` is unambiguous so a global route is safe).
- `Error::Message(_)` is **not** globally network-specific (grit uses it broadly), so the network
  porcelain wraps connect/transfer failures at the call site via a context helper
  `net_err(e) -> PyErr` that maps transport/protocol `Error::Message` and transfer-time `Error::Io`
  to `NetworkError`, while still delegating `Error::Auth`/object/ref variants to `map_err`.

Object/ref/repo errors retain their current mappings.

---

## 6. Progress bridge (GIL & threading)

`http_fetch` / `fetch_remote` take `&mut dyn <progress trait>`. `PyProgressSink` holds an optional
`Py<PyAny>` and, for each trait method, acquires the GIL and calls the matching Python method if the
object defines it (probed via `hasattr`/`getattr`). Because the transfer functions are **not** run
under `py.allow_threads` (callbacks must re-enter Python), the GIL is held across the transfer when a
progress object is supplied; when `progress is None`, `NullProgress` is used and the network/transfer
work can release the GIL (`py.allow_threads`) for blocking I/O. A Python exception raised inside a
sink method propagates out of `clone`/`fetch` (the transfer is abandoned). This trade-off
(GIL-held-with-progress vs GIL-released-without) is documented; a future refinement could marshal
progress events across a channel to release the GIL even with a callback.

---

## 7. Testing (git-oracle, hermetic)

Extends `tests/gitlib.py` + `tests/conftest.py`.

- **git:// (primary, hermetic):** a `git_daemon` fixture launches
  `git daemon --listen=127.0.0.1 --port=<free> --export-all --base-path=<tmp> --reuseaddr`
  in the background, serving an oracle-built bare repo; tear down on fixture exit. Tests:
  - `ls_remote` over `git://127.0.0.1:<port>/repo.git` lists the same refs as `git ls-remote`.
  - `fetch` into a fresh repo writes `refs/remotes/origin/*` + objects; `FetchReport` modes correct.
  - `clone` (worktree) — **parity**: pylibgrit clone vs `git clone` ⇒ identical refs,
    HEAD, and object set (reuse the byte-exact-OID oracle helpers).
  - Skip the whole module if `git daemon` is unavailable.
- **https + auth (`git http-backend`):** a fixture serves a repo via `git http-backend` (CGI behind
  a minimal Python `http.server`), with an optional Basic-auth variant. Tests the http code path,
  `UreqHttpClient`, and the credential provider (401 → retry with creds → success; wrong creds →
  `AuthenticationError`). Skip-if-unavailable, mirroring the existing `git ≥ 2.38` gate.
- **Unit:** scheme dispatch + unknown-scheme `NetworkError`; URL-userinfo parsing/stripping; refspec
  defaulting; progress-sink bridging (a recording sink observes `start/inc/finish`); `origin` config
  contents after clone.
- All **7 existing gates** stay green (`pytest`, `mypy python tests`,
  `python -m mypy.stubtest pylibgrit` with **no allowlist**, `cargo fmt --check`,
  `cargo clippy --all-targets --locked -- -D warnings`, `ruff format --check`, `ruff check`).
  `http-ureq` is compiled into the dev/test build, so clippy/tests cover the http path.

---

## 8. Known limitations & risks (documented)

- **`ls_remote` uses the v0/v1 advertisement.** A server that speaks **only** protocol v2 (empty v1
  advertisement) is unsupported in v1; GitHub/GitLab/`git daemon` all serve a v1 advertisement, so
  this is acceptable. Fetch still negotiates whatever version grit chooses.
- **No ssh, no push, no shallow/depth, no bare/mirror clone** — all deferred (push → Phase D).
- **`FETCH_HEAD`:** the binding writes a git-style `FETCH_HEAD` after a successful fetch **iff**
  grit-lib does not already (pinned during planning by reading `fetch.rs`/`fetch_head.rs`); if grit
  emits it, the binding does not duplicate it.
- **Progress holds the GIL** for the duration of a transfer that supplies a progress object (§6).
- **Credential-helper side effects** (token `store` on success) are intentional and git-faithful.
- No `insteadOf`/url-rewrite, no submodule/promisor handling.
- **Symbol names to pin during planning** (capabilities confirmed; exact identifiers verified
  against the source before coding): the progress trait name (`ProgressSink` vs a re-exported
  `Progress` used in `fetch_remote`'s signature) and the config-writer type (`ConfigFile` /
  `from_path` / `set` / `add_value` / `write`), plus the exact `UpdateMode` variant set.

---

## 9. Load-bearing references (grit-lib 0.4.1, verified)

- `grit_lib::fetch::fetch_remote(local_git_dir: &Path, conn: &mut dyn Connection, opts: &FetchOptions, progress: &mut dyn Progress) -> Result<FetchOutcome>` — does **not** apply ref updates.
- `grit_lib::transport::http::http_fetch(client: &dyn HttpClient, local_git_dir: &Path, repo_url: &str, opts: &FetchOptions, progress: &mut dyn Progress) -> Result<FetchOutcome>` — **self-applies** refs.
- `grit_lib::transport::{Transport, Connection, Service, ConnectOptions, GitDaemonTransport}`;
  `Connection::advertised_refs() -> &[(String, ObjectId)]`, `head_symref() -> Option<&str>`,
  `protocol_version() -> u8`.
- `grit_lib::transport::http::{SmartHttpTransport, HttpClient}`;
  `grit_lib::transport::http::ureq_client::UreqHttpClient` (feature `http-ureq`): `new()`,
  `from_config(&ConfigSet)`, `with_credential_provider(Box<dyn CredentialProvider + Send + Sync>)`.
- `grit_lib::credentials::{Credential, CredentialProvider, HelperCredentialProvider}`;
  `HelperCredentialProvider::new(config: ConfigSet)`; `CredentialProvider::{fill, approve, reject}`.
- `grit_lib::transfer::{FetchOptions (Default), FetchOutcome, RefUpdate, UpdateMode, TagMode}`;
  clone refspec `"+refs/heads/*:refs/remotes/origin/*"`.
- Progress trait in `grit_lib::progress` (`start/inc/set/message/finish`) + `NullProgress`.
- Config writer in `grit_lib::config` (`ConfigFile::from_path` / `set` / `add_value` / `write`).
- `ls_remote::ls_remote` is **local-only** — deliberately **not** used for remote listing.
- Phase A/B primitives reused: `init`, ref writer / `update_ref`, `checkout_tree(update_index=True)`,
  commit/tree reads.

---

## 10. Deliverable

A **0.3.0** feature release: `clone`/`fetch`/`ls_remote` over git:// and https with credentials and
progress, git-faithful clones, all gates green, hermetic oracle parity over git:// and a
`git http-backend` https path. Push, ssh, and shallow remain for later phases.
