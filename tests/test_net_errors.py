"""The two networking exception types exist and subclass GritError."""

import pylibgrit


def test_network_error_is_griterror_subclass() -> None:
    assert issubclass(pylibgrit.NetworkError, pylibgrit.GritError)


def test_authentication_error_is_griterror_subclass() -> None:
    assert issubclass(pylibgrit.AuthenticationError, pylibgrit.GritError)


def test_exceptions_are_distinct() -> None:
    assert pylibgrit.NetworkError is not pylibgrit.AuthenticationError
    assert not issubclass(pylibgrit.NetworkError, pylibgrit.AuthenticationError)
    assert not issubclass(pylibgrit.AuthenticationError, pylibgrit.NetworkError)
