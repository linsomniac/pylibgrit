import pytest

from tests.gitlib import cat_file_data, rev_parse


def test_odb_read_blob_matches_git(simple_repo):
    import pygrit

    blob_oid = rev_parse(simple_repo, "HEAD:a.txt")
    repo = pygrit.Repository.discover(str(simple_repo))
    obj = repo.odb.read(pygrit.ObjectId.from_hex(blob_oid))
    assert obj.id.hex == blob_oid
    assert obj.kind is pygrit.ObjectKind.BLOB
    assert obj.data == cat_file_data(simple_repo, blob_oid)


def test_odb_read_commit_matches_git(simple_repo):
    import pygrit

    commit_oid = rev_parse(simple_repo, "HEAD")
    repo = pygrit.Repository.discover(str(simple_repo))
    obj = repo.odb.read(pygrit.ObjectId.from_hex(commit_oid))
    assert obj.kind is pygrit.ObjectKind.COMMIT
    assert obj.data == cat_file_data(simple_repo, commit_oid)


def test_odb_exists(simple_repo):
    import pygrit

    commit_oid = rev_parse(simple_repo, "HEAD")
    repo = pygrit.Repository.discover(str(simple_repo))
    assert repo.odb.exists(pygrit.ObjectId.from_hex(commit_oid)) is True


def test_odb_read_missing_raises(simple_repo):
    import pygrit

    repo = pygrit.Repository.discover(str(simple_repo))
    missing = pygrit.ObjectId.from_hex("0" * 40)
    with pytest.raises(pygrit.ObjectNotFoundError):
        repo.odb.read(missing)
