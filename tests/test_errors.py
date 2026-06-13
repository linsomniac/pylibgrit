def test_exception_hierarchy():
    import pygrit

    assert issubclass(pygrit.RepositoryError, pygrit.GritError)
    assert issubclass(pygrit.ObjectNotFoundError, pygrit.GritError)
    assert issubclass(pygrit.InvalidObjectError, pygrit.GritError)
    assert pygrit.GritError is not pygrit.RepositoryError
    assert not issubclass(pygrit.ObjectNotFoundError, pygrit.RepositoryError)
