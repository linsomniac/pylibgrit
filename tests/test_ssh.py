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
