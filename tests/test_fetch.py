"""repo.fetch over git:// writes tracking refs + objects and reports updates."""

from __future__ import annotations

import pytest

import pylibgrit


def test_fetch_writes_tracking_refs_and_objects(git_daemon, tmp_path) -> None:
    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(dst)
    report = repo.fetch(git_daemon.repo_url)

    head = pylibgrit.ObjectId.from_hex(git_daemon.head_oid)
    assert repo.odb.exists(head)
    track = repo.resolve("refs/remotes/origin/main")
    assert track.hex == git_daemon.head_oid

    modes = {u.remote_ref: u.mode for u in report.updates}
    assert modes[b"refs/heads/main"] == "new"
    # grit returns the SHORT branch name for the default branch (HEAD symref), not the full ref.
    assert report.default_branch == b"main"


# AIDEV-NOTE: Documents grit-lib 0.4.1's tag-following shared-oid bug as a STRICT xfail: when a tag
# (here v1) points at the same commit as a fetched head, `add_wire_tags` adds that oid to the
# "following-only" set, which the wants filter then excludes — so the head's objects never arrive,
# even though the tracking ref is still written. strict=True flags an xpass if a future grit-lib
# fixes it (revisit then). Workaround for callers: tags="all" or tags="none". See design spec §8.
@pytest.mark.xfail(
    strict=True,
    reason="grit-lib 0.4.1: tags='following' skips a head whose oid is also a tag target "
    "(add_wire_tags poisons the wants set); fixed -> revisit. Workaround: tags='all'/'none'.",
)
def test_fetch_following_drops_head_sharing_tag_oid(
    git_daemon_shared_tag, tmp_path
) -> None:
    repo = pylibgrit.Repository.init(tmp_path / "dst")
    repo.fetch(git_daemon_shared_tag.repo_url)  # default tags="following"
    head = pylibgrit.ObjectId.from_hex(git_daemon_shared_tag.head_oid)
    assert repo.odb.exists(
        head
    )  # SHOULD hold; currently fails due to the grit bug -> xfail


def test_fetch_idempotent_second_is_not_new(git_daemon, tmp_path) -> None:
    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(dst)
    repo.fetch(git_daemon.repo_url)
    report = repo.fetch(git_daemon.repo_url)
    modes = {u.remote_ref: u.mode for u in report.updates}
    assert modes[b"refs/heads/main"] in {"up-to-date", "no-change-needed"}
