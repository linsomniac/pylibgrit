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


def test_commit_index_first_commit_unborn(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"hello\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    idx.write()
    sig = pylibgrit.Signature(b"Test Author", b"author@example.com", (1112911993, 0))
    com = pylibgrit.Signature(
        b"Test Committer", b"committer@example.com", (1112911993, 0)
    )
    oid = repo.commit_index(message=b"initial\n", author=sig, committer=com)
    assert _git(work, git_env, "rev-parse", "refs/heads/main") == oid.hex
    parents = (
        subprocess.run(
            ["git", "rev-list", "--parents", "-n", "1", oid.hex],
            cwd=work,
            env=git_env,
            stdout=subprocess.PIPE,
            check=True,
        )
        .stdout.decode()
        .split()
    )
    assert parents == [oid.hex]  # no parents on the first commit
    reflog = _git(work, git_env, "reflog", "show", "refs/heads/main")
    assert "initial" in reflog


def test_commit_index_advances_with_parent(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))
    b1 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"one\n")
    idx = repo.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.write()
    c1 = repo.commit_index(message=b"one\n", author=sig, committer=sig)
    b2 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"two\n")
    idx.add(b"a.txt", b2, 0o100644)
    idx.write()
    c2 = repo.commit_index(message=b"two\n", author=sig, committer=sig)
    assert _git(work, git_env, "rev-parse", "refs/heads/main") == c2.hex
    parents = (
        subprocess.run(
            ["git", "rev-list", "--parents", "-n", "1", c2.hex],
            cwd=work,
            env=git_env,
            stdout=subprocess.PIPE,
            check=True,
        )
        .stdout.decode()
        .split()
    )
    assert parents == [c2.hex, c1.hex]


def test_commit_index_merge_extra_parents(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))
    b1 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"one\n")
    idx = repo.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.write()
    c1 = repo.commit_index(message=b"one\n", author=sig, committer=sig)
    side = repo.create_commit(
        repo.commit(c1).tree, parents=[c1], author=sig, committer=sig, message=b"side\n"
    )
    b2 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"merged\n")
    idx.add(b"a.txt", b2, 0o100644)
    idx.write()
    merge = repo.commit_index(
        message=b"merge\n", parents=[side], author=sig, committer=sig
    )
    parents = (
        subprocess.run(
            ["git", "rev-list", "--parents", "-n", "1", merge.hex],
            cwd=work,
            env=git_env,
            stdout=subprocess.PIPE,
            check=True,
        )
        .stdout.decode()
        .split()
    )
    assert parents == [merge.hex, c1.hex, side.hex]  # branch tip first, then extra


def test_commit_index_detached_head_raises(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))
    b1 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"one\n")
    idx = repo.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.write()
    repo.commit_index(message=b"one\n", author=sig, committer=sig)
    subprocess.run(
        ["git", "checkout", "-q", "--detach"], cwd=work, env=git_env, check=True
    )
    with pytest.raises(pylibgrit.RepositoryError):
        repo.commit_index(message=b"x\n", author=sig, committer=sig)
