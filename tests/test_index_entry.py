def test_index_entry_minimal():
    import pylibgrit

    oid = pylibgrit.ObjectId.from_hex("0" * 40)
    e = pylibgrit.IndexEntry(b"a.txt", oid, 0o100644)
    assert e.path == b"a.txt"
    assert e.oid == oid
    assert e.mode == 0o100644
    assert e.size == 0
    assert e.ctime == (0, 0)


def test_index_entry_full_fields():
    import pylibgrit

    oid = pylibgrit.ObjectId.from_hex("1" * 40)
    e = pylibgrit.IndexEntry(
        b"src/x", oid, 0o100755,
        ctime=(11, 12), mtime=(13, 14), dev=5, ino=6, uid=7, gid=8, size=9, flags=3,
    )
    assert (e.ctime, e.mtime, e.dev, e.ino, e.uid, e.gid, e.size, e.flags) == (
        (11, 12), (13, 14), 5, 6, 7, 8, 9, 3,
    )
