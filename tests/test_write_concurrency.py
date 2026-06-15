import subprocess
import threading


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def test_parallel_blob_writes_are_sound(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))

    results: dict[int, str] = {}
    errors: list[Exception] = []

    def worker(n: int) -> None:
        try:
            oid = pg.odb.write(pylibgrit.ObjectKind.BLOB, f"content-{n}\n".encode())
            results[n] = oid.hex
        except Exception as exc:  # pragma: no cover - failure path
            errors.append(exc)

    threads = [threading.Thread(target=worker, args=(n,)) for n in range(50)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert not errors
    assert len(set(results.values())) == 50  # distinct contents -> distinct oids
    for n, hexoid in results.items():
        assert pg.odb.read(pylibgrit.ObjectId.from_hex(hexoid)).data == f"content-{n}\n".encode()
