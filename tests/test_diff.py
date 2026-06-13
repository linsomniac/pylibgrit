"""Diff tests: tree/commit diff status, oracle'd against git."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest


@pytest.fixture
def diff_repo(tmp_path: Path, git_env: dict[str, str]) -> Path:
    repo = tmp_path / "diff"
    repo.mkdir()

    def g(*a: str) -> None:
        subprocess.run(["git", *a], cwd=repo, env=git_env, check=True)

    g("init", "-q", "-b", "main")
    (repo / "keep").write_text("a\n")
    (repo / "gone").write_text("b\n")
    g("add", "-A")
    g("commit", "-q", "-m", "base")
    (repo / "keep").write_text("a2\n")  # modify
    (repo / "gone").unlink()  # delete
    (repo / "added").write_text("c\n")  # add
    g("add", "-A")
    g("commit", "-q", "-m", "change")
    return repo


def test_diff_status_matches_git(diff_repo: Path) -> None:
    import pygrit

    from tests.gitlib import run_git

    a = run_git(diff_repo, "rev-parse", "HEAD^").decode().strip()
    b = run_git(diff_repo, "rev-parse", "HEAD").decode().strip()
    # git diff --raw -z: meta record (starts ':') then path(s) as separate \0 fields.
    raw = run_git(diff_repo, "diff", "--raw", "-z", a, b)
    fields = [f for f in raw.split(b"\0") if f]
    expected = {}
    i = 0
    while i < len(fields):
        meta = fields[i]  # e.g. b":100644 100644 <oid> <oid> M"
        status = meta.split(b" ")[-1].decode()
        path = fields[i + 1]
        expected[path] = status[0]
        i += 2
    repo = pygrit.Repository.discover(str(diff_repo))
    d = repo.diff(repo.resolve("HEAD^"), repo.resolve("HEAD"))
    got = {}
    for e in d:
        key = e.old_path if e.status == "D" else e.new_path
        got[key] = e.status
    assert got == expected


def test_diff_len(diff_repo: Path) -> None:
    import pygrit

    repo = pygrit.Repository.discover(str(diff_repo))
    d = repo.diff(repo.resolve("HEAD^"), repo.resolve("HEAD"))
    assert len(d) == 3  # keep modified, gone deleted, added added


def test_diff_iter_outlives_repo(diff_repo: Path) -> None:
    """FFI lifetime: a DiffIter must stay valid after the Diff and Repository drop."""
    import pygrit

    repo = pygrit.Repository.discover(str(diff_repo))
    d = repo.diff(repo.resolve("HEAD^"), repo.resolve("HEAD"))
    it = iter(d)
    del d
    del repo
    statuses = sorted(e.status for e in it)
    assert statuses == ["A", "D", "M"]
