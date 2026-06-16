import subprocess


def _git_text(repo, env, *args):
    return (
        subprocess.run(
            ["git", *args], cwd=repo, env=env, stdout=subprocess.PIPE, check=True
        )
        .stdout.decode()
        .strip()
    )


def test_init_non_bare_recognized_by_git(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work), initial_branch=b"main")
    assert repo.is_bare is False
    assert _git_text(work, git_env, "rev-parse", "--is-bare-repository") == "false"
    assert _git_text(work, git_env, "symbolic-ref", "HEAD") == "refs/heads/main"


def test_init_bare(tmp_path, git_env):
    import pylibgrit

    gd = tmp_path / "b.git"
    repo = pylibgrit.Repository.init(str(gd), bare=True, initial_branch=b"trunk")
    assert repo.is_bare is True
    assert _git_text(gd, git_env, "rev-parse", "--is-bare-repository") == "true"
    assert _git_text(gd, git_env, "symbolic-ref", "HEAD") == "refs/heads/trunk"


def test_init_default_branch_is_main(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "d"
    pylibgrit.Repository.init(str(work))
    assert _git_text(work, git_env, "symbolic-ref", "HEAD") == "refs/heads/main"
