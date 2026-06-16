import subprocess

import pytest


def _git(repo, env, *args):
    return (
        subprocess.run(
            ["git", *args], cwd=repo, env=env, stdout=subprocess.PIPE, check=True
        )
        .stdout.decode()
        .strip()
    )


def _diamond(work, git_env):
    """base -> (A on main) and (B on feat); return (repo_path, oid_A, oid_B, oid_base)."""
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True
    )
    (work / "f.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base = _git(work, git_env, "rev-parse", "HEAD")
    (work / "a.txt").write_text("a\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    oid_a = _git(work, git_env, "rev-parse", "HEAD")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base)
    (work / "b.txt").write_text("b\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    oid_b = _git(work, git_env, "rev-parse", "HEAD")
    return work, oid_a, oid_b, base


def test_merge_base_matches_git(tmp_path, git_env):
    import pylibgrit

    work, a, b, base = _diamond(tmp_path / "r", git_env)
    repo = pylibgrit.Repository.open(str(work / ".git"))
    mb = repo.merge_base(pylibgrit.ObjectId.from_hex(a), pylibgrit.ObjectId.from_hex(b))
    assert mb is not None
    assert mb.hex == base
    assert mb.hex == _git(work, git_env, "merge-base", a, b)


def test_merge_base_unrelated_is_none(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True
    )
    (work / "x").write_text("x\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "one")
    one = _git(work, git_env, "rev-parse", "HEAD")
    _git(work, git_env, "checkout", "-q", "--orphan", "orphan")
    (work / "y").write_text("y\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "two")
    two = _git(work, git_env, "rev-parse", "HEAD")
    repo = pylibgrit.Repository.open(str(work / ".git"))
    assert (
        repo.merge_base(
            pylibgrit.ObjectId.from_hex(one), pylibgrit.ObjectId.from_hex(two)
        )
        is None
    )


def test_merge_trees_clean_matches_git(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True
    )
    (work / "f.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    base_commit = _git(work, git_env, "rev-parse", "HEAD")
    (work / "a.txt").write_text("a\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    ours_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    ours_commit = _git(work, git_env, "rev-parse", "HEAD")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base_commit)
    (work / "b.txt").write_text("b\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    theirs_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    theirs_commit = _git(work, git_env, "rev-parse", "HEAD")

    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_trees(
        pylibgrit.ObjectId.from_hex(base_tree),
        pylibgrit.ObjectId.from_hex(ours_tree),
        pylibgrit.ObjectId.from_hex(theirs_tree),
    )
    assert res.has_conflicts is False
    assert res.conflicts == []
    got = res.write_tree().hex

    oracle = subprocess.run(
        ["git", "merge-tree", "--write-tree", ours_commit, theirs_commit],
        cwd=work,
        env=git_env,
        stdout=subprocess.PIPE,
    )
    if oracle.returncode != 0:
        pytest.skip("git merge-tree --write-tree unavailable (<2.38)")
    assert got == oracle.stdout.decode().strip()


def test_merge_trees_conflict_reports_paths(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True
    )
    (work / "c.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    base_commit = _git(work, git_env, "rev-parse", "HEAD")
    (work / "c.txt").write_text("ours\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    ours_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base_commit)
    (work / "c.txt").write_text("theirs\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    theirs_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")

    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_trees(
        pylibgrit.ObjectId.from_hex(base_tree),
        pylibgrit.ObjectId.from_hex(ours_tree),
        pylibgrit.ObjectId.from_hex(theirs_tree),
    )
    assert res.has_conflicts is True
    assert b"c.txt" in res.conflicts
    assert res.conflict_blob(b"c.txt") is not None
    marker_oid = res.conflict_blob(b"c.txt")
    assert marker_oid is not None
    assert b"<<<<<<<" in repo.blob(marker_oid).data  # marker blob is written + readable
    with pytest.raises(pylibgrit.RepositoryError):
        res.write_tree()


def test_merge_trees_favor_ours(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True
    )
    (work / "c.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    base_commit = _git(work, git_env, "rev-parse", "HEAD")
    (work / "c.txt").write_text("ours\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    ours_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base_commit)
    (work / "c.txt").write_text("theirs\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    theirs_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")

    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_trees(
        pylibgrit.ObjectId.from_hex(base_tree),
        pylibgrit.ObjectId.from_hex(ours_tree),
        pylibgrit.ObjectId.from_hex(theirs_tree),
        favor="ours",
    )
    assert res.has_conflicts is False
    tree = res.write_tree()
    t = repo.tree(tree)
    c_oid = next(e.id for e in t if e.name == b"c.txt")
    assert repo.blob(c_oid).data == b"ours\n"


def test_merge_trees_favor_theirs(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True
    )
    (work / "c.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    base_commit = _git(work, git_env, "rev-parse", "HEAD")
    (work / "c.txt").write_text("ours\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    ours_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base_commit)
    (work / "c.txt").write_text("theirs\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    theirs_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")

    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_trees(
        pylibgrit.ObjectId.from_hex(base_tree),
        pylibgrit.ObjectId.from_hex(ours_tree),
        pylibgrit.ObjectId.from_hex(theirs_tree),
        favor="theirs",
    )
    assert res.has_conflicts is False
    tree = res.write_tree()
    t = repo.tree(tree)
    c_oid = next(e.id for e in t if e.name == b"c.txt")
    assert repo.blob(c_oid).data == b"theirs\n"


def test_merge_trees_bad_favor_raises(tmp_path, git_env):
    import pylibgrit

    work, *_ = _diamond(tmp_path / "r", git_env)
    repo = pylibgrit.Repository.open(str(work / ".git"))
    head_tree = pylibgrit.ObjectId.from_hex(
        _git(work, git_env, "rev-parse", "HEAD^{tree}")
    )
    with pytest.raises(ValueError):
        repo.merge_trees(head_tree, head_tree, head_tree, favor="bogus")
