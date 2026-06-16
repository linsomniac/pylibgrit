import threading

import pytest


def _repo_with_main(tmp_path):
    import pylibgrit

    repo = pylibgrit.Repository.init(str(tmp_path / "r"))
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    idx.write()
    tree = idx.write_tree()
    sig = pylibgrit.Signature(b"A", b"a@x", (1700000000, 0))
    c1 = repo.create_commit(
        tree, parents=[], author=sig, committer=sig, message=b"c1\n"
    )
    repo.update_ref(b"refs/heads/main", c1, create=True)
    blob2 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"y\n")
    idx.add(b"a.txt", blob2, 0o100644)
    tree2 = idx.write_tree()
    c2 = repo.create_commit(
        tree2, parents=[c1], author=sig, committer=sig, message=b"c2\n"
    )
    return repo, c1, c2


def test_cas_mismatch_raises_and_leaves_ref(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    with pytest.raises(pylibgrit.RefMismatchError):
        repo.update_ref(b"refs/heads/main", c2, expected_old=c2)
    assert repo.resolve("refs/heads/main") == c1


def test_cas_success_advances(tmp_path):
    repo, c1, c2 = _repo_with_main(tmp_path)
    repo.update_ref(b"refs/heads/main", c2, expected_old=c1)
    assert repo.resolve("refs/heads/main") == c2


def test_create_only_on_existing_raises(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    with pytest.raises(pylibgrit.RefMismatchError):
        repo.update_ref(b"refs/heads/main", c2, create=True)


def test_preexisting_lock_is_contention_error(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    lock = tmp_path / "r" / ".git" / "refs" / "heads" / "main.lock"
    lock.write_text("")
    with pytest.raises(pylibgrit.RepositoryError):
        repo.update_ref(b"refs/heads/main", c2, expected_old=c1)
    lock.unlink()
    assert not lock.exists()


def test_threaded_race_exactly_one_winner(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    results = []
    barrier = threading.Barrier(8)

    def attempt():
        barrier.wait()
        try:
            repo.update_ref(b"refs/heads/main", c2, expected_old=c1)
            results.append(True)
        except pylibgrit.RefMismatchError:
            results.append(False)
        except pylibgrit.RepositoryError:
            results.append(False)

    threads = [threading.Thread(target=attempt) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert results.count(True) == 1
    assert repo.resolve("refs/heads/main") == c2


def test_cas_delete_loose(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    repo.update_ref(b"refs/tags/v1", c1, create=True)
    with pytest.raises(pylibgrit.RefMismatchError):
        repo.delete_ref(b"refs/tags/v1", expected_old=c2)
    assert repo.resolve("refs/tags/v1") == c1
    repo.delete_ref(b"refs/tags/v1", expected_old=c1)
    with pytest.raises(pylibgrit.GritError):
        repo.resolve("refs/tags/v1")


# AIDEV-NOTE: The rename-failure cleanup in atomic_cas_write (a rename(lock -> ref) failure must
# still remove the `<ref>.lock`) is covered by CODE INSPECTION, not a runtime test. Attempting to
# force the rename to fail by making the ref PATH an (empty or non-empty) directory does NOT reach
# the rename branch: grit's read_raw_ref classifies an existing directory at the ref path as a
# present-but-unresolvable ref (NOT NotFound), so atomic_cas_write fails earlier in the VERIFY step
# (current_under_lock -> resolve_ref -> "ref not found" -> RepositoryError). That verify-error path
# has its own lock cleanup (also verified: no stale lock remains), but it is a DIFFERENT branch than
# the rename fix. The lock and the rename target share a parent directory, so no portable filesystem
# obstruction makes read_raw_ref report NotFound while rename still fails — hence the code fix is
# locked in by inspection rather than a flaky/platform-specific test.
