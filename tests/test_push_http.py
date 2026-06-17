"""repo.push over anonymous smart-HTTP (git http-backend with receive-pack enabled)."""

from __future__ import annotations

import pylibgrit
from tests.gitlib import run_git


def test_http_push(http_push_server) -> None:
    p, env = http_push_server, http_push_server.env
    (p.local_path / "b.txt").write_text("two\n")
    run_git(p.local_path, "add", "-A", env=env)
    run_git(
        p.local_path,
        "-c",
        "user.name=T",
        "-c",
        "user.email=t@e",
        "commit",
        "-q",
        "-m",
        "c2",
        env=env,
    )
    new = run_git(p.local_path, "rev-parse", "HEAD", env=env).decode().strip()
    repo = pylibgrit.Repository.open(p.local_path / ".git", p.local_path)
    report = repo.push(p.repo_url, ["main"])
    assert report.ok
    assert (
        run_git(p.server_path, "rev-parse", "refs/heads/main", env=env).decode().strip()
        == new
    )
