"""Tests for write-core input validation (Fix A–E hardening, codex review #1/#2/#6/#7/#8)."""

from __future__ import annotations

import subprocess

import pytest


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def _open(tmp_path, git_env, work_tree=False):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    if work_tree:
        return pylibgrit.Repository.open(str(repo / ".git"), str(repo)), repo
    return pylibgrit.Repository.open(str(repo / ".git")), repo


# --- Fix A: Signature validation -----------------------------------------
def test_signature_offset_min_does_not_panic(tmp_path, git_env):
    import pylibgrit

    with pytest.raises(ValueError):
        pylibgrit.Signature(b"A", b"a@x", (0, -2147483648))  # i32::MIN: must NOT panic


def test_signature_offset_out_of_range(tmp_path, git_env):
    import pylibgrit

    with pytest.raises(ValueError):
        pylibgrit.Signature(b"A", b"a@x", (0, 90000))  # > 24h


def test_signature_offset_not_minute_aligned(tmp_path, git_env):
    import pylibgrit

    with pytest.raises(ValueError):
        pylibgrit.Signature(b"A", b"a@x", (0, 90))  # 90s not a whole minute


@pytest.mark.parametrize("name", [b"Ada\nEvil", b"a<b", b"a>b", b"a\x00b"])
def test_signature_rejects_delimiters_in_name(name):
    import pylibgrit

    with pytest.raises(ValueError):
        pylibgrit.Signature(name, b"a@x", (0, 0))


def test_signature_valid_still_works():
    import pylibgrit

    s = pylibgrit.Signature(b"Ada", b"ada@x.io", (1718000000, 19800))
    assert s.raw == b"Ada <ada@x.io> 1718000000 +0530"


# --- Fix B: index path validation (incl. the SIGSEGV case) ----------------
def test_index_add_leading_slash_rejected_no_segfault(tmp_path, git_env):
    import pylibgrit

    pg, _ = _open(tmp_path, git_env)
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = pg.index()
    with pytest.raises(ValueError):
        idx.add(
            b"/etc/passwd", blob, 0o100644
        )  # would SIGSEGV in write_tree before the fix


@pytest.mark.parametrize(
    "bad", [b"", b"/abs", b"a/", b"../x", b"a/../b", b"a/./b", b"a//b"]
)
def test_index_add_rejects_bad_paths(tmp_path, git_env, bad):
    import pylibgrit

    pg, _ = _open(tmp_path, git_env)
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = pg.index()
    with pytest.raises(ValueError):
        idx.add(bad, blob, 0o100644)


def test_index_add_entry_validates_path(tmp_path, git_env):
    import pylibgrit

    pg, _ = _open(tmp_path, git_env)
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = pg.index()
    with pytest.raises(ValueError):
        idx.add_entry(pylibgrit.IndexEntry(b"../escape", blob, 0o100644))


def test_stage_rejects_parent_escape(tmp_path, git_env):
    pg, repo = _open(tmp_path, git_env, work_tree=True)
    idx = pg.index()
    with pytest.raises(ValueError):
        idx.stage(b"../outside")


def test_index_add_valid_path_ok(tmp_path, git_env):
    import pylibgrit

    pg, _ = _open(tmp_path, git_env)
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = pg.index()
    idx.add(b"dir/sub/file.txt", blob, 0o100644)  # nested relative path is fine
    assert len(idx) == 1


# --- Fix C: ref name validation ------------------------------------------
@pytest.mark.parametrize(
    "bad", [b"../evil", b"/abs/ref", b"refs/heads/..", b"refs//heads"]
)
def test_update_ref_rejects_bad_names(tmp_path, git_env, bad):
    import pylibgrit

    pg, repo = _open(tmp_path, git_env)
    oid = pg.odb.write(
        pylibgrit.ObjectKind.BLOB, b"x\n"
    )  # any oid; should fail on the NAME first
    with pytest.raises((pylibgrit.RepositoryError, pylibgrit.GritError)):
        pg.update_ref(bad, oid)


def test_set_symbolic_ref_rejects_bad_target(tmp_path, git_env):
    import pylibgrit

    pg, repo = _open(tmp_path, git_env)
    with pytest.raises((pylibgrit.RepositoryError, pylibgrit.GritError)):
        pg.set_symbolic_ref(b"refs/heads/alias", b"../evil")


# --- Fix E: reflog/tag control-char rejection ----------------------------
def test_create_tag_rejects_newline_in_name(tmp_path, git_env):
    import pylibgrit

    pg, repo = _open(tmp_path, git_env)
    # need a real target object
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    sig = pylibgrit.Signature(b"T", b"t@x", (1, 0))
    with pytest.raises(ValueError):
        pg.create_tag(
            blob, pylibgrit.ObjectKind.BLOB, b"v\n1", message=b"m\n", tagger=sig
        )


def test_append_reflog_rejects_newline_message(tmp_path, git_env):
    import pylibgrit

    pg, repo = _open(tmp_path, git_env)
    o = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    sig = pylibgrit.Signature(b"T", b"t@x", (1, 0))
    with pytest.raises(ValueError):
        pg.append_reflog(
            b"refs/heads/main",
            o,
            o,
            signer=sig,
            message=b"line1\nline2",
            force_create=True,
        )
