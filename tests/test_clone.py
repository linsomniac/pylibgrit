"""Repository.clone over git:// produces a git-faithful worktree clone."""

from __future__ import annotations

from pathlib import Path

import pylibgrit
from tests.gitlib import run_git


def _all_refs(repo_dir: Path) -> dict[str, str]:
    # name -> oid; `%(refname) %(objectname)` rows split cleanly on the single space.
    out = run_git(
        repo_dir, "for-each-ref", "--format=%(refname) %(objectname)"
    ).decode()
    refs: dict[str, str] = {}
    for line in out.splitlines():
        name, oid = line.split(" ", 1)
        refs[name] = oid
    return refs


def test_clone_matches_git_clone(git_daemon, tmp_path) -> None:
    ours = tmp_path / "ours"
    theirs = tmp_path / "theirs"
    pylibgrit.Repository.clone(git_daemon.repo_url, ours)
    run_git(tmp_path, "clone", "-q", git_daemon.repo_url, str(theirs))

    assert run_git(ours, "rev-parse", "HEAD") == run_git(theirs, "rev-parse", "HEAD")
    assert (ours / "a.txt").read_text() == "hello\n"
    assert (ours / "dir" / "b.txt").read_text() == "world\n"

    assert _all_refs(ours).get("refs/remotes/origin/main") == git_daemon.head_oid
    assert _all_refs(ours).get("refs/heads/main") == git_daemon.head_oid


def test_clone_writes_origin_config(git_daemon, tmp_path) -> None:
    ours = tmp_path / "ours"
    pylibgrit.Repository.clone(git_daemon.repo_url, ours)
    url = run_git(ours, "config", "remote.origin.url").decode().strip()
    fetch = run_git(ours, "config", "remote.origin.fetch").decode().strip()
    assert url == git_daemon.repo_url
    assert fetch == "+refs/heads/*:refs/remotes/origin/*"


def test_clone_head_is_on_branch(git_daemon, tmp_path) -> None:
    ours = tmp_path / "ours"
    repo = pylibgrit.Repository.clone(git_daemon.repo_url, ours)
    head = repo.head()
    assert head.is_symbolic
    assert head.symbolic_target == b"refs/heads/main"


def test_clone_fetches_tags(git_daemon, tmp_path) -> None:
    # clone uses tags="all" (git clone fetches all tags); v1 is on an older commit.
    ours = tmp_path / "ours"
    pylibgrit.Repository.clone(git_daemon.repo_url, ours)
    assert "refs/tags/v1" in _all_refs(ours)
