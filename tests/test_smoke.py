def test_native_module_imports():
    import pygrit

    assert hasattr(pygrit, "Repository")
    assert hasattr(pygrit, "ObjectId")
    assert "Repository" in pygrit.__all__
