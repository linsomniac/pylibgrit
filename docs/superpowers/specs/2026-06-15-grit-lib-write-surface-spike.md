# grit-lib 0.4.1 Write-Surface Spike

**Date:** 2026-06-15
**Type:** Research spike (read-only investigation; no code produced)
**Purpose:** Map what *mutation / write* capability `grit-lib` 0.4.1 exposes, to scope a
future "write-core v2" for pygrit. pygrit today is **read-core only** (open/discover,
read commit/tree/blob/tag, list/resolve refs, revwalk/log, diff, read config).

## Summary verdict

grit-lib 0.4.1 is **substantially write-capable** — it is a from-scratch full Git
reimplementation, not a read-only viewer. It exposes the complete low-level write
toolchain: write loose objects, build trees from the index, serialize commit/tag
objects, read **and** write the index + stage entries, mutate refs and HEAD, init
repositories, do three-way tree merges, write working-tree files, and even fetch/push
over a caller-supplied transport. pygrit currently calls **none** of these.

The single biggest constraint is **architectural, not capability**: grit-lib ships **no
porcelain that orchestrates a full operation**. There is no `repo.commit(...)`,
`repo.checkout(...)`, or `repo.tag(...)`. The `porcelain::*` modules are almost entirely
*compute-only* ("library computes a model, the CLI applies it"). A write binding must
**assemble each multi-step workflow itself** in Rust
(blob → index → write_tree → serialize_commit → odb.write → write_ref → append_reflog),
exactly as grit-lib's own `examples/commit_tree.rs` and `examples/cherry_pick.rs` do.
Everything is plumbing; the binding builds the porcelain.

## Capability map

| Area | Status | Key public signatures |
| --- | --- | --- |
| **A. Object writing** | EXPOSED | `Odb::write(&self, kind: ObjectKind, data: &[u8]) -> Result<ObjectId>` (+ `write_local`, `write_raw`, `write_raw_local`, `write_loose_materialize`); `Odb::hash(&self, kind, data) -> ObjectId`; `objects::serialize_commit(&CommitData) -> Vec<u8>`, `serialize_tag(&TagData) -> Vec<u8>`, `serialize_tree(&[TreeEntry]) -> Vec<u8>`. |
| **B. Ref mutation** | EXPOSED | `refs::write_ref(git_dir, refname, &ObjectId)`, `refs::write_symbolic_ref(git_dir, refname, target)` (sets HEAD), `refs::delete_ref(git_dir, refname)`, `refs::append_reflog(git_dir, refname, old, new, identity, message, force_create)`. No public transactional/atomic multi-ref update; no public packed-refs writer. |
| **C. Index / staging** | EXPOSED | `Index::new()/load(path)/write(path)`; `Repository::load_index/write_index(&self, &mut Index)`; mutators (`&mut self`): `add_or_replace(IndexEntry)`, `stage_file(IndexEntry)`, `remove(&[u8]) -> bool`, `remove_descendants_under_path`, `sort()`; `write_tree::write_tree_from_index(odb, index, prefix) -> Result<ObjectId>`; `index::entry_from_stat(path, rel_path, oid, mode) -> Result<IndexEntry>`. `IndexEntry` is a 15-field public struct. |
| **D. Commit creation** | EXPOSED (manual) | No `create_commit` helper. Build `CommitData { tree, parents, author, committer, author_raw, committer_raw, encoding, message, raw_message }` → `serialize_commit` → `odb.write(ObjectKind::Commit, &raw)`. Amend = same path with the original parents. |
| **E. Working tree** | PARTIAL (primitives) | `porcelain::checkout::{write_to_worktree, apply_index_file_mode, prepare_parent_dirs_for_checkout, remove_empty_parent_dirs}`. No high-level "checkout a tree into the worktree" orchestrator. `porcelain::stash::apply_stash` is the one full mutating worktree+index op. |
| **F. Porcelain (init/reset/merge/rebase/cherry-pick/tag)** | PARTIAL | `repo::init_repository(path, bare, initial_branch, template_dir, ref_storage) -> Result<Repository>` and `merge_trees::merge_trees_three_way(...)` (the merge engine) are real and EXPOSED. But `porcelain::{merge,rebase,cherry_pick,revert,tag}` are compute-only (no ref/object writes); reset and tag-*creation* are not one-call ops. |
| **G. Networking (fetch/push/clone)** | EXPOSED but low-level | `fetch::fetch_remote(git_dir, &mut dyn Connection, &FetchOptions, &mut dyn Progress)`, `push::push_remote(git_dir, &mut dyn Connection, &[PushRefSpec], opts, progress)`, local `transfer::fetch_local/push_local/build_pack`. No `clone` porcelain. Requires a caller-supplied `dyn Connection`; HTTP is behind the off-by-default `http-ureq` feature. |

## Recommended "write-core v2" minimal surface

Smallest coherent first shipment — a **local object/ref write core**, all thin wrappers
(no algorithm reimplementation in the binding), mirroring `examples/commit_tree.rs`:

1. `Odb.write(kind, data) -> oid` and `Odb.hash(kind, data) -> oid`.
2. Index write path: load/construct `Index`, `add_or_replace`/`remove`, `write_index`.
3. `write_tree_from_index(odb, index, "") -> tree_oid`.
4. Commit-creation helper over `CommitData` + `serialize_commit` + `odb.write`.
5. Ref mutation: `write_ref`, `write_symbolic_ref` (set HEAD), `delete_ref`, optional `append_reflog`.

That gives Python `hash_object`, write blob/tree/commit/tag, stage→index, write-tree,
commit-tree, update-ref — a scriptable "build commits in a (bare or non-bare) repo" API
with no transport or feature dependencies.

**Tier 2 (still backed):** repo `init`, tag-object creation, three-way merge via
`merge_trees_three_way`, worktree-file checkout primitives.

**Deferred / costly upstream:** all porcelain orchestration (commit/checkout/merge/
rebase/reset/tag) is the real v2 work the binding must own; no atomic multi-ref update;
no packed-refs writer; `clone` absent and fetch/push need a transport + `http-ureq`
(likely a separate, much larger phase).

## Binding risk notes

- **`Error` is `#[non_exhaustive]`** — pygrit's existing `map_err` already handles this;
  write ops add no uncatchable new variant categories.
- **`&mut` is on the owned `Index`, never on `Repository`.** `Odb` writes through `&self`
  (interior `Arc<Mutex>`), and the `Index` is a local value the binding owns/mutates/writes
  — so the existing `Arc<grit_lib::repo::Repository>` design needs no `&mut Repository`.
- **On-disk side effects are immediate** (atomic temp-file + rename; loose objects set
  `0o444`; existing objects are *freshened*, not error-on-collision; ref writes use
  `.lock` + rename). Tests must use tempdirs. The only "dry run" is the in-memory overlay
  (`Odb::enable_mem_overlay()`/`disable_mem_overlay()`).
- **Byte-fidelity of hand-built objects.** `CommitData`/`TagData` store identities as raw
  Git-wire strings (`Name <email> <unix> <+HHMM>`) with `author_raw`/`committer_raw:
  Vec<u8>` and `raw_message: Option<Vec<u8>>` escape hatches. A write binding must let
  Python supply these exactly, or produced OIDs won't match git. `IndexEntry` has raw stat
  fields the examples zero out (fine for synthetic commits; use `entry_from_stat` for a
  real file).
- **Thread-safety.** Ref writes take `git_dir: &Path` and rely on filesystem lock files,
  not in-process locks — no cross-thread/process ref-update atomicity. `Odb` is
  `Arc<Mutex>` internally, so GIL-released writes are sound.
- **Hash algorithm.** `Odb::hash`/`write` auto-detect SHA-1 vs SHA-256 from the repo;
  prefer them over the static `Odb::hash_object_data` (hard-wired SHA-1).

## Open questions for a v2 brainstorm

1. **Scope ceiling:** local write-core only (objects + index + trees + commits + refs),
   or also worktree checkout and/or merge? (Recommendation: Tier 1 local-only first.)
2. **`IndexEntry` ergonomics:** expose the raw 15-field struct (max fidelity), a
   high-level `repo.stage(path)` (git-like, reads the real file via `entry_from_stat`),
   or both?
3. **Commit identity:** a structured write-side `Signature(name, email, time, offset)`
   the binding formats into the wire string, or accept raw `"Name <email> <unix> <+HHMM>"`
   strings to guarantee byte-identical OIDs?
4. **Reflog policy:** auto-append reflog entries (git default, needs identity + message),
   or leave reflog an opt-in call?
5. **Safety rails:** any guardrails (refuse worktree writes in a bare repo, refuse to
   clobber an existing ref without a force flag)? grit-lib provides none at `write_ref`.
6. **Networking appetite:** is fetch/push/clone in the v2 horizon at all? If yes it is a
   separate, much larger phase (transport `dyn Connection`, `http-ureq` feature, no clone
   porcelain).

## Load-bearing references

ODB write API `…/grit-lib-0.4.1/src/odb.rs`; canonical write workflow
`…/grit-lib-0.4.1/examples/commit_tree.rs` and `examples/cherry_pick.rs`; data structs
`…/src/objects.rs` (`CommitData`, `TagData`, `TreeEntry`, serializers); refs `…/src/refs.rs`;
index `…/src/index.rs` + `…/src/write_tree.rs`; repo init `…/src/repo.rs`
(`init_repository`); error `…/src/error.rs` (`#[non_exhaustive]`). pygrit's current
read-only surface (the gap baseline): `src/{repository,odb,refs,objects}.rs` and
`docs/superpowers/api-matrix.md` (write functions already cataloged there as "deferred —
not in read-core MVP").
