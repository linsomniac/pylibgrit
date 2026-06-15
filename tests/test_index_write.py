import subprocess


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def _ls_files_stage(repo, env):
    return subprocess.run(
        ["git", "ls-files", "--stage"], cwd=repo, env=env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode()


def test_index_add_and_write_persists(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"hello\n")

    idx = pg.index()
    idx.add(b"a.txt", blob, 0o100644)
    idx.write()

    staged = _ls_files_stage(repo, git_env)
    assert blob.hex in staged
    assert "a.txt" in staged
    assert staged.startswith("100644 ")


def test_index_remove(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = pg.index()
    idx.add(b"a.txt", blob, 0o100644)
    assert idx.remove(b"a.txt") is True
    assert idx.remove(b"a.txt") is False
    idx.write()
    assert _ls_files_stage(repo, git_env).strip() == ""


def test_index_add_entry_raw(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"y\n")
    idx = pg.index()
    idx.add_entry(pylibgrit.IndexEntry(b"b.txt", blob, 0o100644))
    idx.write()
    assert "b.txt" in _ls_files_stage(repo, git_env)


def test_write_tree_matches_git(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"hello\n")

    idx = pg.index()
    idx.add(b"a.txt", blob, 0o100644)
    idx.write()
    tree = idx.write_tree()

    git_tree = subprocess.run(
        ["git", "write-tree"], cwd=repo, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert tree.hex == git_tree


def test_stage_real_file_matches_git(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    (repo / "a.txt").write_text("hello\n")

    pg = pylibgrit.Repository.open(str(repo / ".git"), str(repo))
    idx = pg.index()
    idx.stage(b"a.txt")
    idx.write()
    tree = idx.write_tree()

    subprocess.run(["git", "add", "a.txt"], cwd=repo, env=git_env, check=True)
    git_tree = subprocess.run(
        ["git", "write-tree"], cwd=repo, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert tree.hex == git_tree


def test_stage_executable_bit(tmp_path, git_env):
    import os
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    script = repo / "run.sh"
    script.write_text("#!/bin/sh\n")
    os.chmod(script, 0o755)

    pg = pylibgrit.Repository.open(str(repo / ".git"), str(repo))
    idx = pg.index()
    idx.stage(b"run.sh")
    idx.write()

    staged = _ls_files_stage(repo, git_env)
    assert staged.startswith("100755 ")


def test_stage_bare_repo_raises(tmp_path, git_env):
    import pylibgrit
    import pytest

    repo = tmp_path / "r.git"
    subprocess.run(["git", "init", "-q", "--bare", str(repo)], env=git_env, check=True)
    pg = pylibgrit.Repository.open(str(repo))
    idx = pg.index()
    with pytest.raises(pylibgrit.RepositoryError):
        idx.stage(b"a.txt")


def test_index_len_and_iter(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    b1 = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"1\n")
    b2 = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"2\n")
    idx = pg.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.add(b"b.txt", b2, 0o100644)
    assert len(idx) == 2
    names = sorted(e.path for e in idx)
    assert names == [b"a.txt", b"b.txt"]
