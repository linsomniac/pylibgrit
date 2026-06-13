"""Revwalk / rev-list: order + laziness, oracled against `git rev-list`.

AIDEV-NOTE: `repo.revwalk(start, *, order=None)` precomputes the ordered oids via
grit-lib's batch `rev_list` and yields `Commit` objects lazily (see src/revwalk.rs).
`start` is an `ObjectId` (e.g. `repo.resolve("HEAD")`). The default order is committer-date
reverse-chronological, matching `git rev-list HEAD`.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from tests.gitlib import git_text


@pytest.fixture
def linear_repo(tmp_path: Path, git_env: dict[str, str]) -> Path:
    """A repo with four sequential commits c0..c3 on `main`."""
    repo = tmp_path / "lin"
    repo.mkdir()
    subprocess.run(
        ["git", "init", "-q", "-b", "main"], cwd=repo, env=git_env, check=True
    )
    for i in range(4):
        (repo / "f").write_text(f"{i}\n")
        subprocess.run(["git", "add", "-A"], cwd=repo, env=git_env, check=True)
        subprocess.run(
            ["git", "commit", "-q", "-m", f"c{i}"], cwd=repo, env=git_env, check=True
        )
    return repo


def test_revwalk_matches_rev_list(linear_repo: Path) -> None:
    import pygrit

    expected = git_text(linear_repo, "rev-list", "HEAD").split("\n")
    repo = pygrit.Repository.discover(str(linear_repo))
    head = repo.resolve("HEAD")
    got = [c.id.hex for c in repo.revwalk(head)]
    assert got == expected


def test_revwalk_yields_commits(linear_repo: Path) -> None:
    import pygrit

    repo = pygrit.Repository.discover(str(linear_repo))
    head = repo.resolve("HEAD")
    commits = list(repo.revwalk(head))
    # newest commit first (default reverse-chronological order). grit-lib keeps the body's
    # own trailing LF, so a `-m "c3"` message surfaces as b"c3\n".
    assert commits[0].message_bytes == b"c3\n"
    assert commits[-1].message_bytes == b"c0\n"
