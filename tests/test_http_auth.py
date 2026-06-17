"""Authenticated smart-HTTP: kwargs, URL userinfo, and rejection -> AuthenticationError."""

from __future__ import annotations

import pytest

import pylibgrit

USER, PW = "alice", "s3cret"


def test_auth_clone_with_kwargs(http_auth_server, tmp_path) -> None:
    repo = pylibgrit.Repository.clone(
        http_auth_server.repo_url,
        tmp_path / "ours",
        username=USER,
        password=PW,
        use_credential_helpers=False,
    )
    assert repo.resolve("HEAD").hex == http_auth_server.head_oid


def test_auth_clone_with_url_userinfo(http_auth_server, tmp_path) -> None:
    url = http_auth_server.repo_url.replace("http://", f"http://{USER}:{PW}@")
    repo = pylibgrit.Repository.clone(
        url, tmp_path / "ours", use_credential_helpers=False
    )
    assert repo.resolve("HEAD").hex == http_auth_server.head_oid


def test_auth_missing_credentials_raises(http_auth_server, tmp_path) -> None:
    with pytest.raises(pylibgrit.AuthenticationError):
        pylibgrit.Repository.clone(
            http_auth_server.repo_url, tmp_path / "ours", use_credential_helpers=False
        )


def test_auth_wrong_credentials_raises(http_auth_server) -> None:
    with pytest.raises(pylibgrit.AuthenticationError):
        pylibgrit.ls_remote(
            http_auth_server.repo_url,
            username=USER,
            password="wrong",
            use_credential_helpers=False,
        )
