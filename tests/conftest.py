"""Pytest fixtures: hermetic, deterministic git environment."""

from __future__ import annotations

import contextlib
import socket
import subprocess
import threading
import time
from collections.abc import Iterator
from pathlib import Path
from types import SimpleNamespace

import pytest

from tests import githttp
from tests.gitlib import run_git

DETERMINISTIC_DATE = "2005-04-07T22:13:13"


@pytest.fixture
def git_env(tmp_path: Path) -> dict[str, str]:
    """Isolated git environment: no user/system config, fixed identity, UTC, C locale."""
    home = tmp_path / "home"
    home.mkdir()
    return {
        "HOME": str(home),
        "GIT_CONFIG_GLOBAL": str(home / ".gitconfig"),
        "GIT_CONFIG_NOSYSTEM": "1",
        "TZ": "UTC",
        "LC_ALL": "C",
        "PATH": __import__("os").environ["PATH"],
        "GIT_AUTHOR_NAME": "Test Author",
        "GIT_AUTHOR_EMAIL": "author@example.com",
        "GIT_AUTHOR_DATE": DETERMINISTIC_DATE,
        "GIT_COMMITTER_NAME": "Test Committer",
        "GIT_COMMITTER_EMAIL": "committer@example.com",
        "GIT_COMMITTER_DATE": DETERMINISTIC_DATE,
    }


def _git(repo: Path, env: dict[str, str], *args: str) -> None:
    subprocess.run(
        ["git", *args],
        cwd=repo,
        env=env,
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )


@pytest.fixture
def simple_repo(tmp_path: Path, git_env: dict[str, str]) -> Path:
    """A repo with one commit: a.txt='hello\\n', plus a dir/b.txt."""
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, git_env, "init", "-q", "-b", "main")
    (repo / "a.txt").write_text("hello\n")
    (repo / "dir").mkdir()
    (repo / "dir" / "b.txt").write_text("world\n")
    _git(repo, git_env, "add", "-A")
    _git(repo, git_env, "commit", "-q", "-m", "initial commit")
    return repo


def _free_port() -> int:
    """Grab an ephemeral port by binding to :0 and releasing it."""
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def _wait_port(host: str, port: int, proc: subprocess.Popen, timeout: float) -> bool:
    """Poll until `host:port` accepts a connection or `proc` dies / `timeout` elapses."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            return False  # daemon exited (e.g. `git daemon` unavailable)
        try:
            with socket.create_connection((host, port), timeout=0.25):
                return True
        except OSError:
            time.sleep(0.05)
    return False


# AIDEV-NOTE: Launch a `git daemon` over `base` on a free port, wait for it to accept, and
# guarantee teardown. Shared by the git:// fixtures so the Popen/wait/terminate dance lives in one
# place. `--export-all` serves repos under `--base-path` without a `git-daemon-export-ok` marker.
# Yields the chosen port. Skips the test if the daemon never comes up (e.g. `git daemon` absent);
# the pre-skip `proc.terminate()` is a benign no-op when the daemon already died.
# `receive_pack=True` adds `--enable=receive-pack` so the daemon accepts push (git daemon refuses
# receive-pack by default); used by the `git_daemon_push` fixture.
@contextlib.contextmanager
def _serve_git_daemon(
    base: Path, git_env: dict[str, str], receive_pack: bool = False
) -> Iterator[int]:
    port = _free_port()
    args = [
        "git",
        "daemon",
        "--reuseaddr",
        "--listen=127.0.0.1",
        f"--port={port}",
        f"--base-path={base}",
        "--export-all",
    ]
    if receive_pack:
        # AIDEV-NOTE: git daemon refuses receive-pack (push) unless explicitly enabled.
        args.append("--enable=receive-pack")
    args.append(str(base))
    proc = subprocess.Popen(
        args, env=git_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    try:
        if not _wait_port("127.0.0.1", port, proc, timeout=5.0):
            proc.terminate()
            pytest.skip("git daemon unavailable")
        yield port
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


@pytest.fixture
def git_daemon(tmp_path: Path, git_env: dict[str, str]) -> Iterator[SimpleNamespace]:
    """Serve a seeded bare repo over git:// on localhost. Skips if `git daemon` is unavailable.

    Yields a namespace with `repo_url`, `server_path` (the bare repo), `src`, and `head_oid`.

    AIDEV-NOTE: The `v1` tag is on a SEPARATE commit from `main`'s tip (commit1, "first"), NOT on the
    tip — this is realistic AND avoids grit-lib 0.4.1's `tags="following"` shared-oid bug (a tag on
    the head tip makes tag-following drop the head's objects; see the design spec §8 and the xfail in
    test_fetch.py). `main`'s tip (commit2, "initial commit") therefore has a tree with BOTH a.txt and
    dir/b.txt, which a later clone parity test depends on. `head_oid` is `main`'s tip (commit2).
    """
    base = tmp_path / "srv"
    base.mkdir()
    # Seed a source repo, then make the served bare repo a clone of it.
    src = tmp_path / "src"
    src.mkdir()
    _git(src, git_env, "init", "-q", "-b", "main")
    # commit1: a.txt only; tag v1 points HERE (not at main's tip).
    (src / "a.txt").write_text("hello\n")
    _git(src, git_env, "add", "-A")
    _git(src, git_env, "commit", "-q", "-m", "first")
    _git(src, git_env, "tag", "v1")
    # commit2: add dir/b.txt (a.txt unchanged) -> this becomes main's tip / head_oid.
    (src / "dir").mkdir()
    (src / "dir" / "b.txt").write_text("world\n")
    _git(src, git_env, "add", "-A")
    _git(src, git_env, "commit", "-q", "-m", "initial commit")
    server = base / "server.git"
    _git(tmp_path, git_env, "clone", "-q", "--bare", str(src), str(server))
    head_oid = run_git(src, "rev-parse", "HEAD", env=git_env).decode().strip()

    with _serve_git_daemon(base, git_env) as port:
        yield SimpleNamespace(
            repo_url=f"git://127.0.0.1:{port}/server.git",
            server_path=server,
            src=src,
            head_oid=head_oid,
            env=git_env,
        )


@pytest.fixture
def git_daemon_shared_tag(
    tmp_path: Path, git_env: dict[str, str]
) -> Iterator[SimpleNamespace]:
    """Serve a repo where `v1` shares `main`'s tip oid — reproduces grit-lib's tag-following bug.

    AIDEV-NOTE: Single commit (a.txt="hello\\n") tagged `v1`, so `v1` and `main` point at the SAME
    oid. With the git-faithful default `tags="following"`, grit-lib 0.4.1's `add_wire_tags` poisons
    the wants set for that shared oid and drops `main`'s objects (design spec §8). Exercised by the
    strict-xfail `test_fetch_following_drops_head_sharing_tag_oid`. Skips if `git daemon` is absent.
    """
    base = tmp_path / "srv"
    base.mkdir()
    src = tmp_path / "src"
    src.mkdir()
    _git(src, git_env, "init", "-q", "-b", "main")
    (src / "a.txt").write_text("hello\n")
    _git(src, git_env, "add", "-A")
    _git(src, git_env, "commit", "-q", "-m", "only commit")
    _git(src, git_env, "tag", "v1")  # v1 -> main's tip (shared oid)
    server = base / "server.git"
    _git(tmp_path, git_env, "clone", "-q", "--bare", str(src), str(server))
    head_oid = run_git(src, "rev-parse", "HEAD", env=git_env).decode().strip()

    with _serve_git_daemon(base, git_env) as port:
        yield SimpleNamespace(
            repo_url=f"git://127.0.0.1:{port}/server.git",
            server_path=server,
            src=src,
            head_oid=head_oid,
            env=git_env,
        )


# AIDEV-NOTE: Seed a bare server repo and serve it over smart-HTTP via `tests.githttp` (a threaded
# `git http-backend` bridge). Returns (namespace, shutdown) rather than a context manager so the same
# lifecycle can back both the anonymous `http_server` fixture and (next task) an auth'd variant. The
# server runs on a daemon thread on an ephemeral 127.0.0.1 port; callers MUST invoke the returned
# `shutdown()` (it stops serve_forever, closes the socket, and joins the thread) to avoid leaking the
# thread. `pytest.skip` if the listener cannot bind. `auth=None` means anonymous.
def _make_http_server(
    tmp_path: Path, git_env: dict[str, str], auth: tuple[str, str] | None
):
    """Seed a bare server repo and serve it over smart-HTTP. Returns (namespace, shutdown)."""
    base = tmp_path / "httpsrv"
    base.mkdir()
    src = tmp_path / "httpsrc"
    src.mkdir()
    _git(src, git_env, "init", "-q", "-b", "main")
    (src / "a.txt").write_text("hello\n")
    _git(src, git_env, "add", "-A")
    _git(src, git_env, "commit", "-q", "-m", "initial commit")
    server = base / "server.git"
    _git(tmp_path, git_env, "clone", "-q", "--bare", str(src), str(server))
    head_oid = run_git(src, "rev-parse", "HEAD", env=git_env).decode().strip()

    try:
        httpd = githttp.serve(base, git_env, auth)
    except OSError:
        pytest.skip("could not start http server")
    port = httpd.server_address[1]
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()

    ns = SimpleNamespace(
        repo_url=f"http://127.0.0.1:{port}/server.git",
        head_oid=head_oid,
        server_path=server,
    )

    def shutdown() -> None:
        httpd.shutdown()
        httpd.server_close()
        thread.join(timeout=5)

    return ns, shutdown


def _git_http_backend_available(git_env: dict[str, str]) -> bool:
    """True if `git http-backend` can run here (it's part of git; missing only on odd installs)."""
    try:
        rc = subprocess.run(
            ["git", "http-backend"],
            env=git_env,
            input=b"",
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode
    except OSError:
        return False
    return rc in (0, 1, 2)  # runs (it errors without CGI env, but the binary exists)


@pytest.fixture
def http_server(tmp_path: Path, git_env: dict[str, str]):
    """Anonymous smart-HTTP server (git http-backend). Skips if git http-backend is unavailable."""
    if not _git_http_backend_available(git_env):
        pytest.skip("git http-backend unavailable")
    ns, shutdown = _make_http_server(tmp_path, git_env, auth=None)
    try:
        yield ns
    finally:
        shutdown()


# AIDEV-NOTE: Authenticated variant of http_server using HTTP Basic auth (alice / s3cret). The
# credentials are intentionally fixed/arbitrary test values — not secrets. The same
# _make_http_server helper is reused; `auth=("alice", "s3cret")` makes the githttp handler require
# Authorization: Basic and return 401 WWW-Authenticate for any other request. This fixture is the
# target for Task 8 credential-path coverage (kwargs, URL userinfo, missing creds, wrong creds).
@pytest.fixture
def http_auth_server(tmp_path: Path, git_env: dict[str, str]):
    """Basic-auth smart-HTTP server (user 'alice' / pass 's3cret'). Skips if unavailable."""
    if not _git_http_backend_available(git_env):
        pytest.skip("git http-backend unavailable")
    ns, shutdown = _make_http_server(tmp_path, git_env, auth=("alice", "s3cret"))
    try:
        yield ns
    finally:
        shutdown()


# AIDEV-NOTE: A receive-pack-enabled git:// server (bare) plus a local non-bare clone (the pusher).
# Push tests advance `local_path` (via the git oracle) and push to `repo_url`; the oracle for the
# result is the bare server's refs (`run_git(server_path, "rev-parse", <ref>)`). The bare server is
# safe to push any branch to (no checked-out worktree). `base_oid` is the server's initial main tip.
@pytest.fixture
def git_daemon_push(
    tmp_path: Path, git_env: dict[str, str]
) -> Iterator[SimpleNamespace]:
    """git:// server with receive-pack enabled + a local clone to push from. Skips if no git daemon."""
    base = tmp_path / "psrv"
    base.mkdir()
    src = tmp_path / "psrc"
    src.mkdir()
    _git(src, git_env, "init", "-q", "-b", "main")
    (src / "a.txt").write_text("hello\n")
    _git(src, git_env, "add", "-A")
    _git(src, git_env, "commit", "-q", "-m", "c1")
    server = base / "server.git"
    _git(tmp_path, git_env, "clone", "-q", "--bare", str(src), str(server))
    local = tmp_path / "plocal"
    _git(tmp_path, git_env, "clone", "-q", str(server), str(local))
    base_oid = (
        run_git(server, "rev-parse", "refs/heads/main", env=git_env).decode().strip()
    )

    with _serve_git_daemon(base, git_env, receive_pack=True) as port:
        yield SimpleNamespace(
            repo_url=f"git://127.0.0.1:{port}/server.git",
            server_path=server,
            local_path=local,
            base_oid=base_oid,
            env=git_env,
        )
