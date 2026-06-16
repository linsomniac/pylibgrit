import os
import subprocess

import pytest


def test_write_to_worktree_writes_file(tmp_path):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    repo.write_to_worktree(b"sub/greeting.txt", b"hello\n", 0o100644)
    assert (work / "sub" / "greeting.txt").read_bytes() == b"hello\n"


def test_write_to_worktree_executable_bit(tmp_path):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    repo.write_to_worktree(b"run.sh", b"#!/bin/sh\n", 0o100755)
    assert os.access(work / "run.sh", os.X_OK)


def test_write_to_worktree_bare_raises(tmp_path):
    import pylibgrit

    gd = tmp_path / "b.git"
    repo = pylibgrit.Repository.init(str(gd), bare=True)
    with pytest.raises(pylibgrit.RepositoryError):
        repo.write_to_worktree(b"x.txt", b"y", 0o100644)


def _commit_one_file(work, git_env, path, content):
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True
    )
    (work / path).parent.mkdir(parents=True, exist_ok=True)
    (work / path).write_bytes(content)
    subprocess.run(["git", "add", "-A"], cwd=work, env=git_env, check=True)
    subprocess.run(
        ["git", "commit", "-q", "-m", "c"], cwd=work, env=git_env, check=True
    )
    tree = (
        subprocess.run(
            ["git", "rev-parse", "HEAD^{tree}"],
            cwd=work,
            env=git_env,
            stdout=subprocess.PIPE,
            check=True,
        )
        .stdout.decode()
        .strip()
    )
    return tree


def test_checkout_tree_materializes_files(tmp_path, git_env):
    import pylibgrit

    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(str(dst))
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"alpha\n")
    idx = repo.index()
    idx.add(b"dir/a.txt", blob, 0o100644)
    tree = idx.write_tree()
    repo.checkout_tree(tree)
    assert (dst / "dir" / "a.txt").read_bytes() == b"alpha\n"


def test_checkout_tree_overlay_preserves_untracked(tmp_path):
    import pylibgrit

    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(str(dst))
    (dst / "keep.txt").write_bytes(b"mine\n")
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"alpha\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    repo.checkout_tree(idx.write_tree())
    assert (dst / "keep.txt").read_bytes() == b"mine\n"
    assert (dst / "a.txt").read_bytes() == b"alpha\n"


def test_checkout_tree_no_clobber_without_force(tmp_path):
    import pylibgrit

    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(str(dst))
    (dst / "a.txt").write_bytes(b"existing\n")
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"alpha\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    tree = idx.write_tree()
    with pytest.raises(FileExistsError):
        repo.checkout_tree(tree)
    assert (dst / "a.txt").read_bytes() == b"existing\n"
    repo.checkout_tree(tree, force=True)
    assert (dst / "a.txt").read_bytes() == b"alpha\n"


def test_checkout_tree_updates_index(tmp_path, git_env):
    import pylibgrit

    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(str(dst))
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"alpha\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    repo.checkout_tree(idx.write_tree(), update_index=True)
    staged = subprocess.run(
        ["git", "ls-files", "--stage"],
        cwd=dst,
        env=git_env,
        stdout=subprocess.PIPE,
        check=True,
    ).stdout.decode()
    assert "a.txt" in staged


def test_checkout_tree_bare_raises(tmp_path):
    import pylibgrit

    gd = tmp_path / "b.git"
    repo = pylibgrit.Repository.init(str(gd), bare=True)
    empty = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"x")
    idx = repo.index()
    idx.add(b"a.txt", empty, 0o100644)
    tree = idx.write_tree()
    with pytest.raises(pylibgrit.RepositoryError):
        repo.checkout_tree(tree)
