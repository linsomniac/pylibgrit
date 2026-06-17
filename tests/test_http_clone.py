"""clone/fetch/ls_remote over anonymous smart-HTTP (git http-backend)."""

from __future__ import annotations

import pylibgrit


def test_http_ls_remote(http_server) -> None:
    refs = {r.name for r in pylibgrit.ls_remote(http_server.repo_url)}
    assert b"refs/heads/main" in refs


def test_http_clone(http_server, tmp_path) -> None:
    repo = pylibgrit.Repository.clone(http_server.repo_url, tmp_path / "ours")
    assert repo.resolve("HEAD").hex == http_server.head_oid
    assert (tmp_path / "ours" / "a.txt").read_text() == "hello\n"


def test_http_fetch(http_server, tmp_path) -> None:
    repo = pylibgrit.Repository.init(tmp_path / "dst")
    report = repo.fetch(http_server.repo_url)
    assert {u.remote_ref for u in report.updates} >= {b"refs/heads/main"}
    assert repo.resolve("refs/remotes/origin/main").hex == http_server.head_oid
