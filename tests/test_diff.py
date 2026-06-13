"""Diff tests: tree/commit diff status + diffstat summary, oracle'd against git."""

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


def test_diffstat_matches_git(diff_repo: Path) -> None:
    import pygrit

    from tests.gitlib import run_git

    a = run_git(diff_repo, "rev-parse", "HEAD^").decode().strip()
    b = run_git(diff_repo, "rev-parse", "HEAD").decode().strip()
    numstat = (
        run_git(diff_repo, "diff", "--numstat", a, b).decode().strip().splitlines()
    )
    ins = dele = 0
    files = 0
    for line in numstat:
        added, deleted, _path = line.split("\t", 2)
        files += 1
        if added != "-":
            ins += int(added)
        if deleted != "-":
            dele += int(deleted)
    repo = pygrit.Repository.discover(str(diff_repo))
    stats = repo.diff(repo.resolve("HEAD^"), repo.resolve("HEAD")).stats
    assert stats.files_changed == files
    assert stats.insertions == ins
    assert stats.deletions == dele


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


@pytest.mark.xfail(
    reason="count_changes splits bare \\r as a line break; git --numstat splits on \\n only",
    strict=False,
)
def test_diffstat_bare_cr_diverges_from_git(
    tmp_path: Path, git_env: dict[str, str]
) -> None:
    """Document the known --numstat parity gap for bare-CR-as-content files.

    grit's count_changes (via `similar`) treats a bare `\\r` as a line break, but
    `git --numstat` splits on `\\n` only. For `a\\rb\\n` -> `a\\rb\\rc\\rd\\n` the
    binding counts ins=3/del=1 while git counts ins=1/del=1, so the oracle assertion
    fails (xfail). This test exists to keep the divergence executable and visible.
    """
    import pygrit

    from tests.gitlib import run_git

    repo = tmp_path / "barecr"
    repo.mkdir()

    def g(*a: str) -> None:
        subprocess.run(["git", *a], cwd=repo, env=git_env, check=True)

    g("init", "-q", "-b", "main")
    # Disable autocrlf so the bare CR bytes survive verbatim into the blob.
    g("config", "core.autocrlf", "false")
    (repo / "f").write_bytes(b"a\rb\n")
    g("add", "-A")
    g("commit", "-q", "-m", "base")
    (repo / "f").write_bytes(b"a\rb\rc\rd\n")
    g("add", "-A")
    g("commit", "-q", "-m", "change")

    a = run_git(repo, "rev-parse", "HEAD^").decode().strip()
    b = run_git(repo, "rev-parse", "HEAD").decode().strip()
    numstat = run_git(repo, "diff", "--numstat", a, b).decode().strip().splitlines()
    ins = dele = 0
    for line in numstat:
        added, deleted, _path = line.split("\t", 2)
        if added != "-":
            ins += int(added)
        if deleted != "-":
            dele += int(deleted)

    pyrepo = pygrit.Repository.discover(str(repo))
    stats = pyrepo.diff(pyrepo.resolve("HEAD^"), pyrepo.resolve("HEAD")).stats
    # git: ins=1/del=1; binding (count_changes): ins=3/del=1 -> these differ (xfail).
    assert stats.insertions == ins
    assert stats.deletions == dele
