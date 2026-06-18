"""Phase D push follow-ups: empty-refspec no-op short-circuit and push-options capability check."""

from __future__ import annotations

import pytest

import pylibgrit
from tests.gitlib import run_git


def _commit(local, env, name: str, body: str) -> None:
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


def test_push_empty_refspecs_is_noop_without_connecting(simple_repo) -> None:
    # AIDEV-NOTE: An empty refspec list is a guaranteed no-op, so push must short-circuit BEFORE
    # opening any connection. We prove no connection is attempted by pushing to a deliberately
    # unreachable URL: if push tried to connect it would raise NetworkError; instead we get an empty,
    # successful report. (port 1 is unreachable; the URL is never contacted.)
    repo = pylibgrit.Repository.open(simple_repo / ".git", simple_repo)
    report = repo.push("git://127.0.0.1:1/nonexistent.git", [])
    assert report.ok
    assert len(report.results) == 0


def test_push_options_unsupported_raises(git_daemon_push) -> None:
    # AIDEV-NOTE: The receive-pack daemon does NOT advertise push-options
    # (receive.advertisePushOptions defaults to false), so requesting push_options must surface as a
    # NetworkError (grit returns Error::PushOptionsUnsupported, mapped by net_map_err). A real commit
    # is staged so the push is a genuine update attempt, not an up-to-date short-circuit.
    p, env = git_daemon_push, git_daemon_push.env
    _commit(p.local_path, env, "b.txt", "two\n")
    repo = pylibgrit.Repository.open(p.local_path / ".git", p.local_path)
    with pytest.raises(pylibgrit.NetworkError):
        repo.push(p.repo_url, ["main"], push_options=["ci.skip"])
