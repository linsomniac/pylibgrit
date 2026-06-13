def test_objectkind_members():
    import pygrit

    assert {k.name for k in pygrit.ObjectKind} >= {"COMMIT", "TREE", "BLOB", "TAG"}


def test_objectkind_distinct():
    import pygrit

    assert pygrit.ObjectKind.COMMIT != pygrit.ObjectKind.TREE
