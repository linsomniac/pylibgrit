# pylibgrit SSH Transport Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add SSH (`ssh://`, `git+ssh://`, scp-style `user@host:path`) as a fourth transport for `ls_remote`, `fetch`, `clone`, and `push`, staged as release 0.5.0.

**Architecture:** grit-lib 0.4.1's `SshTransport` spawns an `ssh` subprocess and yields the *same* `Box<dyn Connection>` that git:// produces, so SSH reuses every downstream piece (`fetch_remote`, `push_remote`, advertisement reader, report mapping). Each `match classify(url)` site gains a `Scheme::Ssh` arm that mirrors its `Scheme::Git` arm. The ssh program is injectable via an `ssh_command=` kwarg (a shell command run via `sh -c`, like `GIT_SSH_COMMAND`), which also makes tests hermetic via a fake-ssh shim — no real sshd.

**Tech Stack:** PyO3 0.23 (abi3) over grit-lib 0.4.1; maturin; pytest with a fake-ssh shim fixture.

**Reference spec:** `docs/superpowers/specs/2026-06-17-pylibgrit-ssh-transport-design.md`

**Conventions for every task:**
- Rust changes require a rebuild before pytest: `uv run maturin develop --uv --locked`
- The 7 gates must be green before each commit: `uv run pytest -q`; `uv run mypy python tests`; `uv run python -m mypy.stubtest pylibgrit`; `cargo fmt --check`; `cargo clippy --all-targets --locked -- -D warnings`; `uv run ruff format --check`; `uv run ruff check`
- Connections are `!Send`: always construct **and** consume them inside one `py.allow_threads(...)` closure.

---

## File Structure

- **`src/net_transport.rs`** (modify) — `Scheme::Ssh`, ssh detection in `classify`, `build_ssh_transport`, `ssh_connect`, `ssh_connect_receive`, `reject_creds_for_ssh`.
- **`src/remote.rs`** (modify) — `read_advertisement_ssh`; `ls_remote`/`fetch_raw`/`fetch_method`/`clone_impl` gain `ssh_command` + ssh arms/guards.
- **`src/push.rs`** (modify) — `push_method` gains `ssh_command` + ssh arm/guard.
- **`src/repository.rs`** (modify) — `clone`/`fetch`/`push` kwargs.
- **`python/pylibgrit/__init__.pyi`** (modify) — `ssh_command` in the four stubs.
- **`tests/conftest.py`** (modify) — the `ssh_server` shim fixture.
- **`tests/test_ssh.py`** (create) — all ssh tests.
- **`Cargo.toml`, `Cargo.lock`, `README.md`, `CHANGELOG.md`** (modify) — 0.5.0 release.

---

## Task 1: SSH foundation + ls_remote over ssh

**Files:**
- Modify: `src/net_transport.rs`
- Modify: `src/remote.rs` (`read_advertisement` area ~51-58; `ls_remote` ~64-120; `fetch_raw` match ~217-240; imports line 11)
- Modify: `src/push.rs` (`push_method` match ~352-373; imports line 13)
- Modify: `python/pylibgrit/__init__.pyi` (`ls_remote` ~553-560)
- Modify: `tests/conftest.py` (add fixture)
- Create: `tests/test_ssh.py`

This task introduces `Scheme::Ssh`, so the `fetch_raw` and `push_method` matches must compile; they get **temporary** "not yet implemented" arms (replaced in Tasks 2 and 3). Only `ls_remote` is fully wired here.

- [ ] **Step 1: Add the `ssh_server` fixture to `tests/conftest.py`**

Append this fixture at the end of `tests/conftest.py` (it reuses the module's existing `_git`, `run_git`, `stat`, `Path`, `SimpleNamespace`, `Iterator`, and `pytest` imports — verify `import stat` is present at the top; it is, used by other fixtures):

```python
# AIDEV-NOTE: A hermetic "ssh" server. No real sshd: `ssh_command` is a POSIX shim that ignores the
# host/-p args grit passes and runs the remote git command (the LAST argument, e.g.
# `git-upload-pack '/abs/repo.git'` or `git-receive-pack '...'`) locally against the bare repo. The
# shim prepends `git --exec-path` to PATH so the dashed git-upload-pack/git-receive-pack helpers
# resolve regardless of the ambient PATH. Bare server (safe to push to) + a local non-bare pusher
# clone, mirroring `git_daemon_push`. `repo_url` is an ssh:// URL with an absolute path; `scp_url` is
# the scp-style form of the same repo.
@pytest.fixture
def ssh_server(tmp_path: Path, git_env: dict[str, str]) -> SimpleNamespace:
    """Fake-ssh (shim) server: bare receive-pack-capable repo + a local clone to push from."""
    base = tmp_path / "sshsrv"
    base.mkdir()
    src = tmp_path / "sshsrc"
    src.mkdir()
    _git(src, git_env, "init", "-q", "-b", "main")
    (src / "a.txt").write_text("hello\n")
    _git(src, git_env, "add", "-A")
    _git(src, git_env, "commit", "-q", "-m", "c1")
    server = base / "server.git"
    _git(tmp_path, git_env, "clone", "-q", "--bare", str(src), str(server))
    local = tmp_path / "sshlocal"
    _git(tmp_path, git_env, "clone", "-q", str(server), str(local))
    base_oid = (
        run_git(server, "rev-parse", "refs/heads/main", env=git_env).decode().strip()
    )

    shim = tmp_path / "fake-ssh.sh"
    shim.write_text(
        "#!/bin/sh\n"
        "# Fake ssh for hermetic tests: ignore host/-p; run the remote git command\n"
        "# (the last argument) locally. Put git's exec-path on PATH so the dashed\n"
        "# git-upload-pack / git-receive-pack helpers resolve.\n"
        'PATH="$(git --exec-path):$PATH"\n'
        "export PATH\n"
        "for last; do :; done\n"
        'exec sh -c "$last"\n'
    )
    shim.chmod(shim.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    return SimpleNamespace(
        repo_url=f"ssh://localhost{server}",
        scp_url=f"localhost:{server}",
        ssh_command=str(shim),
        server_path=server,
        local_path=local,
        base_oid=base_oid,
        env=git_env,
    )
```

- [ ] **Step 2: Write the failing test file `tests/test_ssh.py`**

```python
"""SSH transport (ssh:// / git+ssh:// / scp-style) via a hermetic fake-ssh shim."""

from __future__ import annotations

import pytest

import pylibgrit


def test_ls_remote_ssh_with_command(ssh_server) -> None:
    refs = pylibgrit.ls_remote(
        ssh_server.repo_url, ssh_command=ssh_server.ssh_command
    )
    names = {r.name for r in refs}
    assert b"refs/heads/main" in names
    main = next(r for r in refs if r.name == b"refs/heads/main")
    assert main.oid.hex == ssh_server.base_oid


def test_ls_remote_ssh_scp_style(ssh_server) -> None:
    refs = pylibgrit.ls_remote(ssh_server.scp_url, ssh_command=ssh_server.ssh_command)
    assert any(r.name == b"refs/heads/main" for r in refs)


def test_ls_remote_ssh_auto_via_env(ssh_server, monkeypatch) -> None:
    # ssh_command=None -> Auto -> resolves GIT_SSH_COMMAND from the environment.
    monkeypatch.setenv("GIT_SSH_COMMAND", ssh_server.ssh_command)
    refs = pylibgrit.ls_remote(ssh_server.repo_url)
    assert any(r.name == b"refs/heads/main" for r in refs)


def test_ls_remote_ssh_rejects_credentials(ssh_server) -> None:
    with pytest.raises(ValueError):
        pylibgrit.ls_remote(
            ssh_server.repo_url,
            username="bob",
            ssh_command=ssh_server.ssh_command,
        )
```

Note: `RemoteRef` exposes `.name` (bytes) and `.oid` (ObjectId with `.hex`) — confirm against an existing test such as `tests/test_ls_remote.py` if unsure.

- [ ] **Step 3: Run the test to verify it fails**

Run: `uv run pytest tests/test_ssh.py -q`
Expected: FAIL — `ls_remote()` rejects the unknown `ssh_command` kwarg (`TypeError`), or (for the env test) raises `NetworkError` "unsupported transport".

- [ ] **Step 4: Extend `src/net_transport.rs`**

Update the transport import (line 3) to add `SshTransport` and `is_ssh_url`:

```rust
use grit_lib::transport::{
    is_ssh_url, ConnectOptions, Connection, GitDaemonTransport, Service, SshTransport, Transport,
};
```

Add the `PyValueError` import after the `use pyo3::prelude::*;` line:

```rust
use pyo3::exceptions::PyValueError;
```

Add `Ssh` to the `Scheme` enum:

```rust
pub(crate) enum Scheme {
    Git,
    Http,
    Ssh,
}
```

Replace the `classify` body's final `else` so ssh URLs are recognized (after the git:// and http(s):// checks):

```rust
pub(crate) fn classify(url: &str) -> PyResult<Scheme> {
    if url.starts_with("git://") {
        Ok(Scheme::Git)
    } else if url.starts_with("https://") || url.starts_with("http://") {
        Ok(Scheme::Http)
    } else if is_ssh_url(url) {
        Ok(Scheme::Ssh)
    } else {
        Err(network_err(&format!(
            "unsupported transport for URL {url:?}; supported schemes: git://, http://, https://, \
             ssh:// (and scp-style host:path)"
        )))
    }
}
```

Add these functions after `git_connect_receive` (end of file). `build_ssh_transport`/`ssh_connect_receive` are added now but `ssh_connect_receive` is only *used* in Task 3 — so add it in Task 3, not here, to avoid a dead-code clippy failure. Add only `build_ssh_transport`, `ssh_connect`, and `reject_creds_for_ssh` in this task:

```rust
// AIDEV-NOTE: Build the ssh transport. `Some(cmd)` pins a shell command line (run via `sh -c`, like
// GIT_SSH_COMMAND); `None` is Auto — grit resolves $GIT_SSH_COMMAND, then $GIT_SSH, then `ssh`.
fn build_ssh_transport(ssh_command: Option<&str>) -> SshTransport {
    match ssh_command {
        Some(cmd) => SshTransport::with_shell_command(cmd),
        None => SshTransport::new(),
    }
}

// AIDEV-NOTE: Connect an ssh service (git-upload-pack) for ls_remote/fetch. `protocol_version` 1 for
// ls_remote (read a v0/v1 advertisement); 0 for fetch (let the server pick). The returned
// `Box<dyn Connection>` wraps a child process — it is `!Send`, so construct + consume it inside one
// `allow_threads` closure (never cross the boundary), exactly like `git_connect`.
pub(crate) fn ssh_connect(
    url: &str,
    protocol_version: u8,
    ssh_command: Option<&str>,
) -> Result<Box<dyn Connection>, grit_lib::error::Error> {
    let opts = ConnectOptions {
        protocol_version,
        server_options: Vec::new(),
    };
    build_ssh_transport(ssh_command).connect(url, Service::UploadPack, &opts)
}

// AIDEV-NOTE: ssh auth (keys/agent/known_hosts) is the ssh subprocess's job, never pylibgrit's. The
// http-only `username`/`password` kwargs do not apply to ssh URLs; passing either with an ssh URL is
// almost certainly a mistake, so fail loud. `use_credential_helpers` (http-only) is left alone.
pub(crate) fn reject_creds_for_ssh(
    url: &str,
    username: &Option<String>,
    password: &Option<String>,
) -> PyResult<()> {
    if (username.is_some() || password.is_some()) && is_ssh_url(url) {
        return Err(PyValueError::new_err(
            "username/password are not used for ssh URLs; ssh authentication is handled by the ssh \
             program (keys/agent). Put the user in the URL, e.g. ssh://user@host/path.",
        ));
    }
    Ok(())
}
```

- [ ] **Step 5: Wire `ls_remote` + add temporary ssh arms in `src/remote.rs`**

Update the import on line 11:

```rust
use crate::net_transport::{classify, git_connect, reject_creds_for_ssh, ssh_connect, Scheme};
```

Add `read_advertisement_ssh` right after the existing `read_advertisement` function (~line 58):

```rust
fn read_advertisement_ssh(
    url: &str,
    ssh_command: Option<&str>,
) -> Result<(Vec<(String, grit_lib::objects::ObjectId)>, Option<String>), grit_lib::error::Error> {
    let conn = ssh_connect(url, 1, ssh_command)?;
    let refs = conn.advertised_refs().to_vec();
    let head = conn.head_symref().map(str::to_owned);
    Ok((refs, head))
}
```

In `ls_remote`, add `ssh_command` to the `#[pyo3(signature = ...)]` (line 65) and the function params (after `tags: bool`), call the guard, and add the `Scheme::Ssh` arm:

```rust
#[pyfunction]
#[pyo3(signature = (url, *, username=None, password=None, use_credential_helpers=true, heads=false, tags=false, ssh_command=None))]
pub fn ls_remote(
    py: Python<'_>,
    url: String,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
    heads: bool,
    tags: bool,
    ssh_command: Option<String>,
) -> PyResult<Vec<RemoteRef>> {
    reject_creds_for_ssh(&url, &username, &password)?;
    let (advertised, head_target) = match classify(&url)? {
        Scheme::Git => py
            .allow_threads(|| read_advertisement(&url))
            .map_err(net_map_err)?,
        Scheme::Http => crate::net_credentials::http_advertisement(
            py,
            &url,
            username,
            password,
            use_credential_helpers,
        )?,
        Scheme::Ssh => py
            .allow_threads(|| read_advertisement_ssh(&url, ssh_command.as_deref()))
            .map_err(net_map_err)?,
    };
    // ... rest of ls_remote unchanged ...
```

Add a **temporary** ssh arm to the `fetch_raw` match (after the `Scheme::Http` arm, ~line 239). `net_map_err` is already imported; this uses no new imports and is replaced in Task 2:

```rust
        Scheme::Ssh => {
            return Err(net_map_err(grit_lib::error::Error::Message(
                "ssh fetch is not yet implemented in this build".to_owned(),
            )))
        }
```

- [ ] **Step 6: Add a temporary ssh arm to `push_method` in `src/push.rs`**

In the `push_method` match (after the `Scheme::Http` arm, ~line 372), add a temporary arm (replaced in Task 3; `net_map_err` is already imported on line 12):

```rust
        Scheme::Ssh => {
            return Err(net_map_err(grit_lib::error::Error::Message(
                "ssh push is not yet implemented in this build".to_owned(),
            )))
        }
```

- [ ] **Step 7: Update the `ls_remote` stub in `python/pylibgrit/__init__.pyi`**

Add `ssh_command: str | None = None` as the last parameter of the `ls_remote` stub (after `tags: bool = False`):

```python
def ls_remote(
    url: str,
    *,
    username: str | None = None,
    password: str | None = None,
    use_credential_helpers: bool = True,
    heads: bool = False,
    tags: bool = False,
    ssh_command: str | None = None,
) -> list[RemoteRef]: ...
```

- [ ] **Step 8: Rebuild and run the test to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ssh.py -q`
Expected: PASS (4 passed).

- [ ] **Step 9: Run all gates**

Run: `uv run pytest -q && uv run mypy python tests && uv run python -m mypy.stubtest pylibgrit && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && uv run ruff format --check && uv run ruff check`
Expected: all green.

- [ ] **Step 10: Commit**

```bash
git add src/net_transport.rs src/remote.rs src/push.rs python/pylibgrit/__init__.pyi tests/conftest.py tests/test_ssh.py
git commit -m "feat: ssh transport foundation + ls_remote over ssh

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: fetch & clone over ssh

**Files:**
- Modify: `src/remote.rs` (`fetch_raw` ~208-242; `fetch_method` ~294-317; `clone_impl` ~376-400)
- Modify: `src/repository.rs` (`clone` ~125-148; `fetch` ~1006-1031)
- Modify: `python/pylibgrit/__init__.pyi` (`clone`, `fetch` stubs)
- Modify: `tests/test_ssh.py`

- [ ] **Step 1: Write the failing tests (append to `tests/test_ssh.py`)**

```python
def _commit(local, env, name: str, body: str) -> None:
    from tests.gitlib import run_git

    (local / name).write_text(body)
    run_git(local, "add", "-A", env=env)
    run_git(
        local,
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@e",
        "commit",
        "-q",
        "-m",
        body.strip(),
        env=env,
    )


def test_clone_over_ssh(ssh_server, tmp_path) -> None:
    dest = tmp_path / "cloned"
    repo = pylibgrit.Repository.clone(
        ssh_server.repo_url, dest, ssh_command=ssh_server.ssh_command
    )
    assert (dest / "a.txt").read_text() == "hello\n"
    head = repo.resolve("refs/heads/main")
    assert head.hex == ssh_server.base_oid


def test_fetch_over_ssh(ssh_server) -> None:
    from tests.gitlib import run_git

    _commit(ssh_server.local_path, ssh_server.env, "b.txt", "two\n")
    run_git(ssh_server.local_path, "push", "-q", "origin", "main", env=ssh_server.env)
    new = (
        run_git(ssh_server.server_path, "rev-parse", "refs/heads/main", env=ssh_server.env)
        .decode()
        .strip()
    )
    repo = pylibgrit.Repository.open(
        ssh_server.local_path / ".git", ssh_server.local_path
    )
    report = repo.fetch(ssh_server.repo_url, ssh_command=ssh_server.ssh_command)
    tip = repo.resolve("refs/remotes/origin/main")
    assert tip.hex == new


def test_clone_over_ssh_rejects_credentials(ssh_server, tmp_path) -> None:
    with pytest.raises(ValueError):
        pylibgrit.Repository.clone(
            ssh_server.repo_url,
            tmp_path / "x",
            password="secret",
            ssh_command=ssh_server.ssh_command,
        )
    assert not (tmp_path / "x").exists()  # guard fires before init (no side effect)


def test_fetch_over_ssh_rejects_credentials(ssh_server) -> None:
    repo = pylibgrit.Repository.open(
        ssh_server.local_path / ".git", ssh_server.local_path
    )
    with pytest.raises(ValueError):
        repo.fetch(
            ssh_server.repo_url,
            username="bob",
            ssh_command=ssh_server.ssh_command,
        )
```

Note: `repo.resolve(spec: str) -> ObjectId` takes a **str** ref name (not bytes) and returns an `ObjectId` whose `.hex` is the hex string — see `tests/test_fetch.py` (`repo.resolve("refs/remotes/origin/main")`).

- [ ] **Step 2: Run to verify failure**

Run: `uv run pytest tests/test_ssh.py -k "ssh and (clone or fetch)" -q`
Expected: FAIL — `clone()`/`fetch()` reject the unknown `ssh_command` kwarg (`TypeError`); without the kwarg they would hit the temporary "ssh fetch is not yet implemented" `NetworkError`.

- [ ] **Step 3: Replace `fetch_raw`'s temporary arm and add the `ssh_command` param (`src/remote.rs`)**

Add `ssh_command: Option<String>` as the last parameter of `fetch_raw` (after `use_credential_helpers: bool`):

```rust
pub(crate) fn fetch_raw(
    py: Python<'_>,
    git_dir: &std::path::Path,
    url: &str,
    opts: &FetchOptions,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
    ssh_command: Option<String>,
) -> PyResult<FetchOutcome> {
```

Replace the temporary `Scheme::Ssh` arm (from Task 1) with the real one, mirroring the `Scheme::Git` arm:

```rust
        Scheme::Ssh => py
            .allow_threads(|| -> Result<FetchOutcome, grit_lib::error::Error> {
                let mut conn = ssh_connect(url, 0, ssh_command.as_deref())?;
                let mut np = grit_lib::fetch::NoProgress;
                grit_lib::fetch::fetch_remote(git_dir, &mut *conn, opts, &mut np)
            })
            .map_err(net_map_err)?,
```

- [ ] **Step 4: Thread `ssh_command` through `fetch_method` and `clone_impl` (`src/remote.rs`)**

In `fetch_method`, add the param, the guard, and pass it down:

```rust
pub(crate) fn fetch_method(
    py: Python<'_>,
    repo: &Arc<grit_lib::repo::Repository>,
    url: String,
    refspecs: Option<Vec<String>>,
    tags: &str,
    prune: bool,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
    ssh_command: Option<String>,
) -> PyResult<FetchReport> {
    reject_creds_for_ssh(&url, &username, &password)?;
    let opts = build_fetch_options(refspecs, tags, prune)?;
    let git_dir = repo.git_dir.clone();
    let outcome = fetch_raw(
        py,
        &git_dir,
        &url,
        &opts,
        username,
        password,
        use_credential_helpers,
        ssh_command,
    )?;
    build_report(py, outcome)
}
```

In `clone_impl`, add the param, move the guard to the fail-fast point (before init), and pass it to `fetch_raw`:

```rust
pub(crate) fn clone_impl(
    py: Python<'_>,
    url: String,
    path: std::path::PathBuf,
    branch: Option<String>,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
    ssh_command: Option<String>,
) -> PyResult<crate::repository::Repository> {
    reject_creds_for_ssh(&url, &username, &password)?;
    classify(&url)?; // fail fast on an unsupported scheme before touching the filesystem
```

and the internal `fetch_raw` call (step 3 inside `clone_impl`):

```rust
    let outcome = fetch_raw(
        py,
        &git_dir,
        &url,
        &opts,
        username,
        password,
        use_credential_helpers,
        ssh_command,
    )?;
```

- [ ] **Step 5: Add the `ssh_command` kwarg to `clone` and `fetch` in `src/repository.rs`**

`clone` (signature line ~126 and params + delegation):

```rust
    #[pyo3(signature = (url, path, *, branch=None, username=None, password=None,
                        use_credential_helpers=true, ssh_command=None))]
    #[allow(clippy::too_many_arguments)]
    fn clone(
        py: Python<'_>,
        url: String,
        path: &Bound<'_, PyAny>,
        branch: Option<String>,
        username: Option<String>,
        password: Option<String>,
        use_credential_helpers: bool,
        ssh_command: Option<String>,
    ) -> PyResult<Self> {
        let path = extract_path(path)?;
        crate::remote::clone_impl(
            py,
            url,
            path,
            branch,
            username,
            password,
            use_credential_helpers,
            ssh_command,
        )
    }
```

`fetch` (signature line ~1006 and params + delegation):

```rust
    #[pyo3(signature = (url, refspecs=None, *, tags="following", prune=false,
                        username=None, password=None, use_credential_helpers=true,
                        ssh_command=None))]
    #[allow(clippy::too_many_arguments)]
    fn fetch(
        &self,
        py: Python<'_>,
        url: String,
        refspecs: Option<Vec<String>>,
        tags: &str,
        prune: bool,
        username: Option<String>,
        password: Option<String>,
        use_credential_helpers: bool,
        ssh_command: Option<String>,
    ) -> PyResult<crate::remote::FetchReport> {
        crate::remote::fetch_method(
            py,
            &self.inner,
            url,
            refspecs,
            tags,
            prune,
            username,
            password,
            use_credential_helpers,
            ssh_command,
        )
    }
```

- [ ] **Step 6: Update the `clone` and `fetch` stubs in `python/pylibgrit/__init__.pyi`**

Add `ssh_command: str | None = None` as the last keyword parameter of both the `clone` and `fetch` stubs (after their `use_credential_helpers: bool = True`).

- [ ] **Step 7: Rebuild and verify the tests pass**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ssh.py -q`
Expected: PASS (8 passed total).

- [ ] **Step 8: Run all gates** (same command as Task 1 Step 9). Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add src/remote.rs src/repository.rs python/pylibgrit/__init__.pyi tests/test_ssh.py
git commit -m "feat: fetch & clone over ssh

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: push over ssh

**Files:**
- Modify: `src/net_transport.rs` (add `ssh_connect_receive`)
- Modify: `src/push.rs` (`push_method` ~320-375; import line 13)
- Modify: `src/repository.rs` (`push` ~1037-1069)
- Modify: `python/pylibgrit/__init__.pyi` (`push` stub)
- Modify: `tests/test_ssh.py`

- [ ] **Step 1: Write the failing tests (append to `tests/test_ssh.py`)**

```python
def test_push_over_ssh(ssh_server) -> None:
    from tests.gitlib import run_git

    _commit(ssh_server.local_path, ssh_server.env, "c.txt", "three\n")
    new = (
        run_git(ssh_server.local_path, "rev-parse", "HEAD", env=ssh_server.env)
        .decode()
        .strip()
    )
    repo = pylibgrit.Repository.open(
        ssh_server.local_path / ".git", ssh_server.local_path
    )
    report = repo.push(
        ssh_server.repo_url, ["main"], ssh_command=ssh_server.ssh_command
    )
    assert report.ok
    server_tip = (
        run_git(ssh_server.server_path, "rev-parse", "refs/heads/main", env=ssh_server.env)
        .decode()
        .strip()
    )
    assert server_tip == new


def test_push_over_ssh_rejects_credentials(ssh_server) -> None:
    repo = pylibgrit.Repository.open(
        ssh_server.local_path / ".git", ssh_server.local_path
    )
    with pytest.raises(ValueError):
        repo.push(
            ssh_server.repo_url,
            ["main"],
            username="bob",
            ssh_command=ssh_server.ssh_command,
        )
```

- [ ] **Step 2: Run to verify failure**

Run: `uv run pytest tests/test_ssh.py -k push -q`
Expected: FAIL — `push()` rejects the unknown `ssh_command` kwarg (`TypeError`); without it the push would hit the temporary "ssh push is not yet implemented" `NetworkError`.

- [ ] **Step 3: Add `ssh_connect_receive` to `src/net_transport.rs`**

Add after `ssh_connect` (it is now used by `push_method`, so no dead-code warning):

```rust
// AIDEV-NOTE: Connect an ssh service (git-receive-pack) for push. Forces protocol v0 (grit's push
// rejects v2), like `git_connect_receive`. The returned `Box<dyn Connection>` wraps a child process
// (`!Send`) — construct + consume it inside one `allow_threads` closure.
pub(crate) fn ssh_connect_receive(
    url: &str,
    ssh_command: Option<&str>,
) -> Result<Box<dyn Connection>, grit_lib::error::Error> {
    let opts = ConnectOptions {
        protocol_version: 0,
        server_options: Vec::new(),
    };
    build_ssh_transport(ssh_command).connect(url, Service::ReceivePack, &opts)
}
```

- [ ] **Step 4: Wire `push_method` in `src/push.rs`**

Update the import on line 13:

```rust
use crate::net_transport::{
    classify, git_connect_receive, reject_creds_for_ssh, ssh_connect_receive, Scheme,
};
```

Add `ssh_command: Option<String>` as the last parameter of `push_method` (after `progress: Option<Py<PyAny>>`), add the guard as the **first** statement (before `build_push_specs`, so creds are rejected even when `refspecs` is empty), and replace the temporary `Scheme::Ssh` arm with the real one mirroring `Scheme::Git`:

```rust
pub(crate) fn push_method(
    py: Python<'_>,
    repo: &Arc<grit_lib::repo::Repository>,
    url: String,
    refspecs: Vec<Py<PyAny>>,
    force: bool,
    atomic: bool,
    dry_run: bool,
    push_options: Option<Vec<String>>,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
    progress: Option<Py<PyAny>>,
    ssh_command: Option<String>,
) -> PyResult<PushReport> {
    reject_creds_for_ssh(&url, &username, &password)?;
    let specs = build_push_specs(py, repo, refspecs, force)?;
    // ... empty short-circuit + opts + git_dir + prog unchanged ...
```

and the real arm (replacing the temporary one):

```rust
        Scheme::Ssh => {
            let result = py.allow_threads(|| -> Result<PushOutcome, grit_lib::error::Error> {
                let mut conn = ssh_connect_receive(&url, ssh_command.as_deref())?;
                grit_lib::push::push_remote(&git_dir, &mut *conn, &specs, &opts, &mut prog)
            });
            if let Some(e) = prog.take_error() {
                return Err(e);
            }
            result.map_err(net_map_err)?
        }
```

Note: `username`/`password` remain consumed only by the `Scheme::Http` arm; `reject_creds_for_ssh` borrows them before the match, which is fine.

- [ ] **Step 5: Add the `ssh_command` kwarg to `push` in `src/repository.rs`**

```rust
    #[pyo3(signature = (url, refspecs, *, force=false, atomic=false, dry_run=false,
                        push_options=None, username=None, password=None,
                        use_credential_helpers=true, progress=None, ssh_command=None))]
    #[allow(clippy::too_many_arguments)]
    fn push(
        &self,
        py: Python<'_>,
        url: String,
        refspecs: Vec<Py<PyAny>>,
        force: bool,
        atomic: bool,
        dry_run: bool,
        push_options: Option<Vec<String>>,
        username: Option<String>,
        password: Option<String>,
        use_credential_helpers: bool,
        progress: Option<Py<PyAny>>,
        ssh_command: Option<String>,
    ) -> PyResult<crate::push::PushReport> {
        crate::push::push_method(
            py,
            &self.inner,
            url,
            refspecs,
            force,
            atomic,
            dry_run,
            push_options,
            username,
            password,
            use_credential_helpers,
            progress,
            ssh_command,
        )
    }
```

- [ ] **Step 6: Update the `push` stub in `python/pylibgrit/__init__.pyi`**

Add `ssh_command: str | None = None` as the last keyword parameter of the `push` stub (after `progress: ...`).

- [ ] **Step 7: Rebuild and verify the tests pass**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ssh.py -q`
Expected: PASS (10 passed total).

- [ ] **Step 8: Run all gates** (same command as Task 1 Step 9). Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add src/net_transport.rs src/push.rs src/repository.rs python/pylibgrit/__init__.pyi tests/test_ssh.py
git commit -m "feat: push over ssh

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Release 0.5.0 (version + docs)

**Files:**
- Modify: `Cargo.toml` (version line 3)
- Modify: `Cargo.lock` (the `pylibgrit` package version)
- Modify: `README.md` (the "### Supported transports" subsection ~line 245)
- Modify: `CHANGELOG.md` (new top entry)

- [ ] **Step 1: Bump the version in `Cargo.toml`**

Change line 3 from `version = "0.4.0"` to:

```toml
version = "0.5.0"
```

- [ ] **Step 2: Update `Cargo.lock`**

Run: `cargo build`
This regenerates the `pylibgrit` entry in `Cargo.lock` to 0.5.0. Verify with
`grep -A1 'name = "pylibgrit"' Cargo.lock` showing `version = "0.5.0"`.

- [ ] **Step 3: Add SSH to the README "### Supported transports" subsection**

Read `README.md` around line 245 first, then add an `ssh://` bullet alongside the existing git:// / https bullets and a short paragraph. Insert after the existing transports list:

```markdown
- **`ssh://`, `git+ssh://`, and scp-style `user@host:path`** — pylibgrit spawns the
  system `ssh` (no embedded SSH library). Authentication (keys, ssh-agent, `known_hosts`,
  `~/.ssh/config`) is entirely `ssh`'s job; put the user in the URL (`ssh://user@host/...`).
  The `username=`/`password=` kwargs do **not** apply to ssh URLs and raise `ValueError`.

  The ssh program is configurable per call with `ssh_command=` — a shell command line run
  via `sh -c`, exactly like Git's `GIT_SSH_COMMAND`
  (e.g. `ssh_command="ssh -i ~/.ssh/id_ed25519 -o StrictHostKeyChecking=no"`). When omitted,
  pylibgrit follows Git's default precedence: `$GIT_SSH_COMMAND`, then `$GIT_SSH`, then `ssh`.
  `ls_remote`, `clone`, `fetch`, and `push` all accept `ssh_command=`.
```

- [ ] **Step 4: Add the CHANGELOG entry**

Insert above the `## [0.4.0] - 2026-06-17` entry in `CHANGELOG.md`:

```markdown
## [0.5.0] - 2026-06-17

### Added

- **SSH transport** — `ls_remote`, `clone`, `fetch`, and `push` now support `ssh://`,
  `git+ssh://`, and scp-style `user@host:path` URLs. pylibgrit spawns the system `ssh`
  (no embedded SSH library); authentication (keys, ssh-agent, `known_hosts`) is handled
  entirely by `ssh`.
  - New `ssh_command=` keyword on all four entry points: a shell command line run via
    `sh -c`, like Git's `GIT_SSH_COMMAND`
    (e.g. `"ssh -i ~/.ssh/id_ed25519"`). When omitted, follows Git's precedence
    (`$GIT_SSH_COMMAND` → `$GIT_SSH` → `ssh`).
  - The http-only `username=`/`password=` kwargs raise `ValueError` when used with an ssh
    URL (ssh auth is out of band).

```

- [ ] **Step 5: Rebuild and run the full suite**

Run: `uv run maturin develop --uv --locked && uv run pytest -q`
Expected: PASS (all tests; the package builds as 0.5.0).

- [ ] **Step 6: Run all gates** (same command as Task 1 Step 9). Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock README.md CHANGELOG.md
git commit -m "docs: document ssh transport; stage 0.5.0

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final Review

After all four tasks, dispatch a holistic code review of the whole branch (diff against `main`) before finishing:
- Spec coverage: ssh:// / git+ssh:// / scp-style across ls_remote/fetch/clone/push; `ssh_command=` (Some + Auto); creds rejection; hermetic shim tests; 0.5.0 staged.
- No temporary "not yet implemented" arms remain (replaced in Tasks 2 & 3).
- All `Scheme::Ssh` arms construct + consume the `!Send` connection inside one `allow_threads`.
- Gates green; `mypy.stubtest` passes with no allowlist (all four stubs updated).
