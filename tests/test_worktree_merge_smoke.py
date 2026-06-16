import subprocess


def _git(repo, env, *args):
    return (
        subprocess.run(
            ["git", *args], cwd=repo, env=env, stdout=subprocess.PIPE, check=True
        )
        .stdout.decode()
        .strip()
    )


def test_init_commit_checkout_merge_end_to_end(tmp_path, git_env):
    """init -> stage -> commit_index -> checkout -> branch -> merge -> commit merge."""
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work), initial_branch=b"main")
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))

    # base commit on main
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"base\n")
    idx = repo.index()
    idx.add(b"f.txt", blob, 0o100644)
    idx.write()
    base = repo.commit_index(message=b"base\n", author=sig, committer=sig)

    # materialize the work tree, git agrees it is the committed content
    repo.checkout_tree(repo.commit(base).tree)
    assert (work / "f.txt").read_bytes() == b"base\n"

    # ours: add a.txt on main
    ba = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"a\n")
    idx.add(b"a.txt", ba, 0o100644)
    idx.write()
    ours = repo.commit_index(message=b"A\n", author=sig, committer=sig)

    # theirs: a side branch off base that adds b.txt
    repo.update_ref(b"refs/heads/feat", base, create=True)
    bb = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"b\n")
    fidx = repo.index()
    fidx.add(b"f.txt", blob, 0o100644)
    fidx.add(b"b.txt", bb, 0o100644)
    theirs_tree = fidx.write_tree()
    theirs = repo.create_commit(
        theirs_tree, parents=[base], author=sig, committer=sig, message=b"B\n"
    )
    repo.update_ref(b"refs/heads/feat", theirs, expected_old=base)

    # merge feat into main (clean)
    res = repo.merge_commits(ours, theirs)
    assert res.has_conflicts is False
    merged_tree = res.write_tree()

    # write the merged tree into the index, then commit the merge on main
    repo.checkout_tree(merged_tree, force=True, update_index=True)
    merge_commit = repo.commit_index(
        message=b"merge feat\n", parents=[theirs], author=sig, committer=sig
    )

    # git sees a real merge commit with both parents on main
    assert _git(work, git_env, "rev-parse", "refs/heads/main") == merge_commit.hex
    parents = (
        subprocess.run(
            ["git", "rev-list", "--parents", "-n", "1", merge_commit.hex],
            cwd=work,
            env=git_env,
            stdout=subprocess.PIPE,
            check=True,
        )
        .stdout.decode()
        .split()
    )
    assert parents == [merge_commit.hex, ours.hex, theirs.hex]
    names = _git(work, git_env, "ls-tree", "--name-only", merge_commit.hex).split()
    assert set(names) == {"a.txt", "b.txt", "f.txt"}
