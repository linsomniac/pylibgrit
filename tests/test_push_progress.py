"""The push progress callback receives the remote's side-band-2 (hook) output."""

from __future__ import annotations

import stat

import pytest

import pylibgrit
from tests.gitlib import run_git

MARKER = b"hello-from-hook"


# AIDEV-NOTE: post-receive runs after a successful ref update; receive-pack relays its stdout to
# the client on side-band channel 2 (the "remote: ..." stream).  The hook writes a known sentinel
# line so we can assert the progress callback received the side-band-2 payload.
def _install_hook(server_path) -> None:
    hook = server_path / "hooks" / "post-receive"
    hook.write_text("#!/bin/sh\necho hello-from-hook\n")
    hook.chmod(hook.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _advance(local, env) -> None:
    (local / "b.txt").write_text("two\n")
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
        "c2",
        env=env,
    )


def test_push_progress_receives_hook_output(git_daemon_push) -> None:
    # AIDEV-NOTE: This test proves that grit's push side-band-2 demuxing reaches our PyProgress
    # bridge. The post-receive hook emits "hello-from-hook" on stdout; receive-pack relays that on
    # channel 2; grit calls Progress::message() with the raw bytes; PyProgress calls the Python
    # callback. Assert at least one chunk contains the marker — a vacuous (zero-chunk) pass would
    # mean the progress callback never fired, which would be a real finding about grit's handling.
    p, env = git_daemon_push, git_daemon_push.env
    _install_hook(p.server_path)
    _advance(p.local_path, env)
    chunks: list[bytes] = []
    repo = pylibgrit.Repository.open(p.local_path / ".git", p.local_path)
    report = repo.push(p.repo_url, ["main"], progress=chunks.append)
    assert report.ok
    assert any(MARKER in c for c in chunks), f"expected hook output in {chunks!r}"


def test_push_progress_callback_exception_propagates(git_daemon_push) -> None:
    # AIDEV-NOTE: If the progress callback raises, PyProgress captures the exception and
    # push_method re-raises it after the transfer completes (the push itself is treated as failed).
    p, env = git_daemon_push, git_daemon_push.env
    _install_hook(p.server_path)
    _advance(p.local_path, env)

    class Boom(Exception):
        pass

    def cb(_data: bytes) -> None:
        raise Boom("stop")

    repo = pylibgrit.Repository.open(p.local_path / ".git", p.local_path)
    with pytest.raises(Boom):
        repo.push(p.repo_url, ["main"], progress=cb)


def test_push_progress_callback_exception_propagates_http(http_push_server) -> None:
    # AIDEV-NOTE: The git:// arm is covered above; this asserts the IDENTICAL take_error() path over
    # https (push_http). The post-receive hook guarantees side-band-2 output so the callback fires;
    # when it raises, push_method re-raises the captured exception after the transfer.
    p, env = http_push_server, http_push_server.env
    _install_hook(p.server_path)
    _advance(p.local_path, env)

    class Boom(Exception):
        pass

    def cb(_data: bytes) -> None:
        raise Boom("stop")

    repo = pylibgrit.Repository.open(p.local_path / ".git", p.local_path)
    with pytest.raises(Boom):
        repo.push(p.repo_url, ["main"], progress=cb)
