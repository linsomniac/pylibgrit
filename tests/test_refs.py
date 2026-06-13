"""References + HEAD tests, oracled against real git."""

from __future__ import annotations

from pathlib import Path

from tests.gitlib import run_git


def test_references_match_git(simple_repo: Path) -> None:
    import pygrit

    # AIDEV-NOTE: git 2.53's `for-each-ref` does not accept `-z`, so we split on newline.
    # Ref names and the `%(objectname) %(refname)` format never contain newlines, so this
    # is unambiguous here.
    raw = run_git(simple_repo, "for-each-ref", "--format=%(objectname) %(refname)")
    expected = {}
    for rec in raw.split(b"\n"):
        if not rec:
            continue
        oid, name = rec.split(b" ", 1)
        expected[name] = oid.decode()
    repo = pygrit.Repository.discover(str(simple_repo))
    got = {r.name: r.target.hex for r in repo.references() if r.target is not None}
    assert got == expected
