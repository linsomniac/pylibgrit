"""The git_daemon_push fixture serves a receive-pack-enabled bare repo (oracle: git push works)."""

from __future__ import annotations

from tests.gitlib import run_git


def test_oracle_push_works(git_daemon_push) -> None:
    # The git CLI (oracle) can push a new commit to the served bare repo over git://.
    local = git_daemon_push.local_path
    env = git_daemon_push.env
    (local / "b.txt").write_text("two\n")
    run_git(local, "add", "-A", env=env)
    run_git(
        local,
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
    run_git(local, "push", "-q", git_daemon_push.repo_url, "main", env=env)
    server_main = (
        run_git(git_daemon_push.server_path, "rev-parse", "refs/heads/main", env=env)
        .decode()
        .strip()
    )
    local_main = (
        run_git(local, "rev-parse", "refs/heads/main", env=env).decode().strip()
    )
    assert server_main == local_main
