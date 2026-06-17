"""ls_remote over git:// matches `git ls-remote` and supports filters."""

from __future__ import annotations

import pytest

import pylibgrit
from tests.gitlib import run_git


def _oracle_refs(repo_dir, url) -> dict[str, str]:
    # name -> oid for non-peeled rows of `git ls-remote`.
    out = run_git(repo_dir, "ls-remote", url).decode()
    refs = {}
    for line in out.splitlines():
        oid, name = line.split("\t")
        if name.endswith("^{}"):
            continue
        refs[name] = oid
    return refs


def test_ls_remote_matches_oracle(git_daemon, tmp_path) -> None:
    oracle = _oracle_refs(tmp_path, git_daemon.repo_url)
    got = {r.name.decode(): r.oid.hex for r in pylibgrit.ls_remote(git_daemon.repo_url)}
    assert got["refs/heads/main"] == oracle["refs/heads/main"]
    assert got["refs/tags/v1"] == oracle["refs/tags/v1"]
    assert "HEAD" in got
    assert got["HEAD"] == oracle["HEAD"]


def test_ls_remote_head_symref(git_daemon) -> None:
    head = next(
        r for r in pylibgrit.ls_remote(git_daemon.repo_url) if r.name == b"HEAD"
    )
    assert head.symref_target == b"refs/heads/main"


def test_ls_remote_heads_filter(git_daemon) -> None:
    names = {r.name for r in pylibgrit.ls_remote(git_daemon.repo_url, heads=True)}
    assert names == {b"refs/heads/main"}


def test_ls_remote_tags_filter(git_daemon) -> None:
    names = {r.name for r in pylibgrit.ls_remote(git_daemon.repo_url, tags=True)}
    assert names == {b"refs/tags/v1"}


def test_ls_remote_unsupported_scheme_raises() -> None:
    with pytest.raises(pylibgrit.NetworkError):
        pylibgrit.ls_remote("ssh://example.com/repo.git")
