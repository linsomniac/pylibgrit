"""SSH transport (ssh:// / git+ssh:// / scp-style) via a hermetic fake-ssh shim."""

from __future__ import annotations

import pytest

import pylibgrit


def test_ls_remote_ssh_with_command(ssh_server) -> None:
    refs = pylibgrit.ls_remote(ssh_server.repo_url, ssh_command=ssh_server.ssh_command)
    names = {r.name for r in refs}
    assert b"refs/heads/main" in names
    main = next(r for r in refs if r.name == b"refs/heads/main")
    assert main.oid.hex == ssh_server.base_oid


def test_ls_remote_ssh_scp_style(ssh_server) -> None:
    refs = pylibgrit.ls_remote(ssh_server.scp_url, ssh_command=ssh_server.ssh_command)
    assert any(r.name == b"refs/heads/main" for r in refs)


def test_ls_remote_ssh_auto_via_env(ssh_server, monkeypatch) -> None:
    # ssh_command=None -> Auto -> resolves GIT_SSH_COMMAND from the environment.
    monkeypatch.setenv("GIT_SSH_COMMAND", ssh_server.ssh_command)
    refs = pylibgrit.ls_remote(ssh_server.repo_url)
    assert any(r.name == b"refs/heads/main" for r in refs)


def test_ls_remote_ssh_rejects_credentials(ssh_server) -> None:
    with pytest.raises(ValueError):
        pylibgrit.ls_remote(
            ssh_server.repo_url,
            username="bob",
            ssh_command=ssh_server.ssh_command,
        )


def _commit(local, env, name: str, body: str) -> None:
    from tests.gitlib import run_git

    (local / name).write_text(body)
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
        body.strip(),
        env=env,
    )


def test_clone_over_ssh(ssh_server, tmp_path) -> None:
    dest = tmp_path / "cloned"
    repo = pylibgrit.Repository.clone(
        ssh_server.repo_url, dest, ssh_command=ssh_server.ssh_command
    )
    assert (dest / "a.txt").read_text() == "hello\n"
    head = repo.resolve("refs/heads/main")
    assert head.hex == ssh_server.base_oid


def test_fetch_over_ssh(ssh_server) -> None:
    from tests.gitlib import run_git

    _commit(ssh_server.local_path, ssh_server.env, "b.txt", "two\n")
    run_git(ssh_server.local_path, "push", "-q", "origin", "main", env=ssh_server.env)
    new = (
        run_git(
            ssh_server.server_path, "rev-parse", "refs/heads/main", env=ssh_server.env
        )
        .decode()
        .strip()
    )
    repo = pylibgrit.Repository.open(
        ssh_server.local_path / ".git", ssh_server.local_path
    )
    repo.fetch(ssh_server.repo_url, ssh_command=ssh_server.ssh_command)
    tip = repo.resolve("refs/remotes/origin/main")
    assert tip.hex == new


def test_clone_over_ssh_rejects_credentials(ssh_server, tmp_path) -> None:
    with pytest.raises(ValueError):
        pylibgrit.Repository.clone(
            ssh_server.repo_url,
            tmp_path / "x",
            password="secret",
            ssh_command=ssh_server.ssh_command,
        )
    assert not (tmp_path / "x").exists()  # guard fires before init (no side effect)


def test_fetch_over_ssh_rejects_credentials(ssh_server) -> None:
    repo = pylibgrit.Repository.open(
        ssh_server.local_path / ".git", ssh_server.local_path
    )
    with pytest.raises(ValueError):
        repo.fetch(
            ssh_server.repo_url,
            username="bob",
            ssh_command=ssh_server.ssh_command,
        )


def test_push_over_ssh(ssh_server) -> None:
    from tests.gitlib import run_git

    _commit(ssh_server.local_path, ssh_server.env, "c.txt", "three\n")
    new = (
        run_git(ssh_server.local_path, "rev-parse", "HEAD", env=ssh_server.env)
        .decode()
        .strip()
    )
    repo = pylibgrit.Repository.open(
        ssh_server.local_path / ".git", ssh_server.local_path
    )
    report = repo.push(
        ssh_server.repo_url, ["main"], ssh_command=ssh_server.ssh_command
    )
    assert report.ok
    server_tip = (
        run_git(
            ssh_server.server_path, "rev-parse", "refs/heads/main", env=ssh_server.env
        )
        .decode()
        .strip()
    )
    assert server_tip == new


def test_push_over_ssh_rejects_credentials(ssh_server) -> None:
    repo = pylibgrit.Repository.open(
        ssh_server.local_path / ".git", ssh_server.local_path
    )
    with pytest.raises(ValueError):
        repo.push(
            ssh_server.repo_url,
            ["main"],
            username="bob",
            ssh_command=ssh_server.ssh_command,
        )
