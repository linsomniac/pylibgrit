# pylibgrit Phase D — Push & Write-Networking — Design

**Status:** Approved (2026-06-17)
**Depends on:** Phase A (write-core 0.2.0) + Phase B (worktree & merge) + Phase C (read-path networking 0.3.0).
**Roadmap:** the final leg of A→B→C→D. Read-path (`ls_remote`/`fetch`/`clone`) shipped in Phase C;
this spec adds the write/push path.

---

## 1. Goal & scope

Add `repo.push(...)` over **git://** and **https** using grit-lib 0.4.1's `push_remote`/`push_http`
(git-receive-pack, protocol **v0/v1 only** — grit rejects v2 push). Reuses Phase C's transport +
credential machinery; grit builds the pack and parses `report-status` internally, so the binding only
assembles ref updates and maps the outcome.

**In scope:** `repo.push` accepting git-style **string refspecs** and a structured **`PushSpec`**;
force, delete, **force-with-lease** (`expected_old`/`expect_absent`); `atomic`, `dry_run`,
`push_options`; an optional **progress** callback (push's side-band-2 actually fires); credentials
reused from Phase C (kwargs / URL userinfo / git credential helpers).

**Deferred (explicit non-goals):**
- **ssh transport** (spawns the system `ssh`; hard to test hermetically).
- **signed push** (`--signed`) — grit's `push_cert` helpers exist but are **not** wired into
  `push_remote`/`push_http`; no `signed` field on `PushOptions`.
- **submodule push** (`push_submodules.rs` is not called by the push entry points).
- **protocol v2 push** (grit explicitly rejects it).

### Decisions (locked during brainstorming)

| Topic | Decision |
| --- | --- |
| API shape | **Both** — git-style string refspecs *and* a structured `PushSpec` escape hatch. |
| Force-with-lease | **Included** — `expected_old` + `expect_absent` on `PushSpec`. |
| Progress | **Exposed** — optional `bytes` callback; the bridge removed in Phase C is re-introduced (it fires for push). |
| Rejection | **Return `PushReport`** (inspect per-ref `status`); raise only on transport/auth/protocol errors. |
| Transports | git:// + https; ssh deferred. |
| `PushSpec.dst` | `bytes` (house-style consistent with the rest of the ref surface). |
| Options | `atomic`, `dry_run`, `push_options` passthrough. |

---

## 2. Public Python API

```python
repo.push(
    url: str,
    refspecs: list[str | PushSpec],          # "main", "+a:b", ":refs/heads/old" (delete), or PushSpec
    *, force: bool = False,                  # default force for string refspecs lacking a leading '+'
    atomic: bool = False,
    dry_run: bool = False,
    push_options: list[str] | None = None,
    username: str | None = None,
    password: str | None = None,
    use_credential_helpers: bool = True,
    progress: Callable[[bytes], None] | None = None,
) -> PushReport
```

### Value objects

- **`PushSpec`** — constructable input pyclass:
  ```python
  PushSpec(
      dst: bytes,                       # destination ref on the remote, e.g. b"refs/heads/main"
      *, src: ObjectId | None = None,   # local object to push; None ⇒ delete
      force: bool = False,
      delete: bool = False,
      expected_old: ObjectId | None = None,   # force-with-lease: remote ref must equal this
      expect_absent: bool = False,            # lease: remote ref must not exist (create-only)
  )
  ```
- **`PushRefResult`** (frozen) — `local_ref: bytes | None`, `remote_ref: bytes`,
  `old_oid: ObjectId | None`, `new_oid: ObjectId | None`, `forced: bool`, `deletion: bool`,
  `status: str`, `message: str | None`. `status` is the lower-kebab name of grit's `PushRefStatus`,
  exactly one of:
  `"up-to-date" | "ok" | "reject-non-fast-forward" | "reject-already-exists" |
  "reject-fetch-first" | "reject-needs-force" | "reject-stale" | "remote-rejected" |
  "atomic-push-failed"`.
- **`PushReport`** (frozen) — `results: list[PushRefResult]`; `ok: bool` (property: every result is
  `"ok"` or `"up-to-date"`).

### Types & encodings (match Phase C)

- **Ref names** (`PushSpec.dst`, `PushRefResult.local_ref`/`remote_ref`): `bytes`. grit's `PushRefSpec.dst`
  is a `String`, so the binding converts `dst` bytes → UTF-8 (non-UTF-8 `dst` raises `ValueError`).
- **OIDs** (`src`, `expected_old`, `old_oid`, `new_oid`): `ObjectId` objects.
- **`status`** / **`message`**: `str`.
- **Refspec strings** in `refspecs`: `str` — a DSL layer, exactly like Phase C's `fetch(refspecs=[…])`.

---

## 3. Architecture

A new `src/push.rs` porcelain dispatches by URL scheme and reuses Phase C infrastructure. grit's push is
a **one-round-trip** operation (advertisement → command-block + pack → `report-status`); the caller
supplies only the ref updates.

### 3.1 Refspec → `PushRefSpec` assembly

Each `refspecs` item becomes a `grit_lib::transfer::PushRefSpec`:
- **`str` item:** `grit_lib::refspec::parse_push_refspec(s) -> RefspecItem{ force, src: Option<String>,
  dst: Option<String>, … }`. Then:
  - `delete` ⇔ `src` is empty (e.g. `":refs/heads/old"`).
  - non-delete: resolve `src` (a local ref/rev) to an `ObjectId` via `grit_lib::rev_parse::resolve_revision`;
    `dst` defaults to the **qualified source ref** when omitted (e.g. `"main"` → push
    `refs/heads/main` → `refs/heads/main`).
  - `force` ⇐ the refspec's leading `+` **or** the method-level `force=` kwarg.
  - `expected_old`/`expect_absent` are **not** expressible in a string (always `None`/`false`).
- **`PushSpec` item:** used directly (`src` is already an `ObjectId`; `dst` bytes → UTF-8 String; lease
  fields passed through). The effective `force` is `PushSpec.force ∨ force=`.

A bare-oid `src` with no `dst` (can't infer a destination) is a `ValueError`.

### 3.2 Scheme dispatch

- **`git://…`** → `net_transport` connect with **`Service::ReceivePack`** (generalize `git_connect` to
  take a `Service`) → `grit_lib::push::push_remote(git_dir, &mut *conn, &refs, &opts, &mut prog)`. The
  `!Send` `Box<dyn Connection>` is constructed and consumed inside one `allow_threads` closure.
- **`https://…` / `http://…`** → reuse `net_credentials::build_http_client` (userinfo split + creds) →
  `grit_lib::push::push_http(&client, git_dir, &clean_url, &refs, &opts, &mut prog)`.
- **Unknown scheme** → `NetworkError` (via the existing `classify`).

`grit_lib::transfer::PushOptions{ atomic, dry_run, push_options }` carries the kwargs. The result
`PushOutcome{ results: Vec<PushRefResult> }` maps to the `PushReport` value object.

### 3.3 Modules

| File | Responsibility |
| --- | --- |
| `src/push.rs` (new) | `PushSpec`/`PushRefResult`/`PushReport` pyclasses; refspec → `PushRefSpec` assembly; scheme dispatch; `PushRefStatus`→str; `push_method`. |
| `src/net_progress.rs` (re-introduced) | `PyProgress`: optional `Py<PyAny>` `bytes` callback → `grit_lib::fetch::Progress` (fires for push). |
| `src/net_transport.rs` (modify) | `git_connect` gains a `Service` parameter (UploadPack for fetch/ls-remote, ReceivePack for push). |
| `src/net_credentials.rs` (reuse) | `build_http_client` unchanged — `push_http` takes the same `HttpClient`. |
| `src/repository.rs` (modify) | `Repository.push` (thin delegator). |
| `src/lib.rs` (modify) | `mod push;` + `mod net_progress;`; register `PushSpec`/`PushRefResult`/`PushReport`. |

---

## 4. Error handling & progress

- **Rejections are data, not exceptions.** A non-fast-forward / lease-stale / already-exists /
  hook-declined ref comes back as a `PushRefResult` with the matching `status` (and `message` for
  `remote-rejected`, carrying the server's `ng <ref> <reason>` text). `repo.push` returns the
  `PushReport` regardless; callers check `report.ok` or per-ref `status`. Only **transport / auth /
  protocol** failures raise: `NetworkError` (transport/protocol), `AuthenticationError` (HTTP 401).
  `grit_lib::error::Error::PushOptionsUnsupported` (server lacks the `push-options` capability) →
  `NetworkError` (extend the network error mapping).
- **Progress** (`PyProgress`, restored from Phase C history): the transfer runs under `allow_threads`
  (GIL released); `message` re-acquires the GIL via `Python::with_gil` per band-2 chunk and calls
  `progress(chunk: bytes)`; a callback exception is captured and re-raised after the transfer returns.
  grit's push caps are `report-status report-status-v2 quiet` **+ `side-band-64k`** (appended when the
  server advertises it, `push.rs:612`); `quiet` suppresses the server's transfer counters, so the
  callback receives the remote's **hook/diagnostic output** (`remote: …` lines, rejection narration) —
  not byte counters. `progress=None` → `grit_lib::fetch::NoProgress` and the GIL stays released.

---

## 5. Testing (git-oracle, hermetic)

Extends `tests/conftest.py` + `tests/githttp.py`.

- **git:// (primary):** a receive-pack-enabled daemon fixture (`git daemon --enable=receive-pack`
  serving a writable bare repo). **Oracle** = the server repo's refs after a push
  (`git -C server.git rev-parse <ref>` / `for-each-ref`). Cases:
  - create a new branch; fast-forward an existing branch; **delete** (`":refs/heads/x"`);
  - `force` a non-fast-forward; **rejected** non-ff (no force → `"reject-non-fast-forward"`, server
    unchanged); **lease** stale (`expected_old` ≠ actual → `"reject-stale"`);
  - `dry_run` (report computed, server unchanged); `atomic` (one bad ref ⇒ all `"atomic-push-failed"`,
    server unchanged); `PushReport.ok` true on clean success.
- **https:** `git http-backend` with receive-pack enabled (`http.receivepack=true` on the served bare
  repo, or the equivalent service flag); anonymous **and** Basic-auth push (reuse the Phase C
  `http_server`/`http_auth_server` machinery + `_make_http_server` with a push-enabled repo).
- **progress (non-vacuous):** install a `pre-receive`/`post-receive` hook in the server bare repo that
  writes a known line to stderr; assert the `progress` callback receives ≥1 chunk containing it
  (receive-pack relays hook output on band-2). Unlike fetch, this fires deterministically.
- **Unit:** refspec parsing (`"main"`, `"+a:b"`, `":x"` delete, bare-oid-without-dst → `ValueError`);
  `PushSpec` direct use incl. lease; status→string mapping; `PushReport.ok`.
- All **7 existing gates** stay green. Both git:// and https push are hermetically testable here.

---

## 6. Packaging & limitations

- **0.4.0** feature release (push completes the A→B→C→D roadmap). No new dependencies (`http-ureq`
  already bundled). README "Networking" section + `CHANGELOG [0.4.0]` updated; `release.yml` untouched.
- **Limitations (documented):** v0/v1 push only (grit rejects v2); no ssh / signed / submodule push;
  string refspecs can't express force-with-lease (use `PushSpec`); `force=`/leading-`+` is plain force,
  `--force-with-lease` only via `PushSpec`; the progress callback carries remote **hook/diagnostic**
  text (the `quiet` cap suppresses server transfer counters).

---

## 7. Load-bearing references (grit-lib 0.4.1, verified)

- `grit_lib::push::push_remote(local_git_dir: &Path, conn: &mut dyn Connection, refs: &[PushRefSpec], opts: &PushOptions, progress: &mut dyn Progress) -> Result<PushOutcome>` (git://, v0/v1; v2 → `Error::Message`).
- `grit_lib::push::push_http(client: &dyn HttpClient, local_git_dir: &Path, repo_url: &str, refs: &[PushRefSpec], opts: &PushOptions, progress: &mut dyn Progress) -> Result<PushOutcome>` (https). Both build the pack internally (`build_pack`) and parse `report-status`.
- `grit_lib::transfer::PushRefSpec{ src: Option<ObjectId>, dst: String, force: bool, delete: bool, expected_old: Option<ObjectId>, expect_absent: bool }`.
- `grit_lib::transfer::PushOptions{ atomic: bool, dry_run: bool, push_options: Vec<String> }` (Default).
- `grit_lib::transfer::PushOutcome{ results: Vec<PushRefResult> }`;
  `grit_lib::push_report::PushRefResult{ local_ref: Option<String>, remote_ref: String, old_oid: Option<ObjectId>, new_oid: Option<ObjectId>, forced: bool, deletion: bool, status: PushRefStatus, message: Option<String> }`.
- `grit_lib::push_report::PushRefStatus` variants: `UpToDate, Ok, RejectNonFastForward, RejectAlreadyExists, RejectFetchFirst, RejectNeedsForce, RejectStale, RemoteRejected, AtomicPushFailed`.
- `grit_lib::refspec::parse_push_refspec(&str) -> Result<RefspecItem, RefspecError>`; `RefspecItem{ force, negative, matching, pattern, exact_sha1, src: Option<String>, dst: Option<String> }`.
- `grit_lib::transport::Service::ReceivePack`; same `Transport::connect` / `GitDaemonTransport` / `SmartHttpTransport` / `UreqHttpClient` as Phase C.
- Push caps `report-status report-status-v2 quiet` + `side-band-64k` (server-advertised, `push.rs:612`); band-2 demuxed to `progress`.
- Reused Phase C primitives: `net_transport::{classify, git_connect, split_userinfo}`, `net_credentials::build_http_client`, the `NetworkError`/`AuthenticationError` mapping, `rev_parse::resolve_revision` / `refs::resolve_ref`.

---

## 8. Deliverable

A **0.4.0** release: `repo.push(url, refspecs, …) -> PushReport` over git:// and https with force /
delete / force-with-lease / atomic / dry-run / push-options, credentials, and a working progress
callback; rejections surfaced as data; hermetic oracle tests over git:// and `git http-backend`
(incl. a hook-driven progress test). Completes the A→B→C→D networking roadmap. ssh / signed / v2 push
remain future work.
