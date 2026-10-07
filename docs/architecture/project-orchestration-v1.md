# Herdr Project Orchestration v1

Herdr Project Orchestration is the project control plane. Herdr remains the execution substrate: sessions, machines, panes, and agent processes. This program owns project identity, coordinator binding, thread records, and recovery.

The fork this document was written against is `eliasstravik/herdr-projects` at `4e4548c` (Release 0.2.34).

## 1. Current architecture

A project is the directory `~/.herdr-projects/<slug>/`. The slug is the folder name. `PROJECT.md` holds settings and instructions. `.state/project.json` holds lifecycle and former slugs. `.state/coordinator.json` holds the last coordinator pane. `threads/t-NNNN.toml` is the thread record.

A coordinator is any agent whose working directory is that folder (`src/coordinator.rs`). `open` focuses the most recently changed agent in the folder. The ticker nudges that same way.

`thread start` writes the TOML record, then asks `herdr worktree create` for a checkout. That command places the worktree under `~/.herdr/worktrees/<repo>/<branch>` and does not take a path (`docs/herdr-notes.md`). `herdr worktree open --cwd <repo> --path <path>` can open a checkout that already exists.

The ticker is the only path that launches an agent and delivers the brief. A merged pull request does not by itself resolve a thread. A dirty worktree is never force-removed.

Profiles, allow-lists, and the rule that the coordinator cannot pass launch flags already exist.

## 2. Problems

The slug and the folder are the project id. The working directory is the coordinator id. There is no workspace record separate from the thread. A pull request is one string on the thread, and the parser accepts only `https://github.com/<owner>/<repo>/pull/<n>`. Attention words are easy to confuse with lifecycle. Operational truth is split across Markdown, TOML, and JSON.

## 3. Reference findings

Cursor Projects contributes the long-lived coordinator, shared context files, and subscriptions. Claude Code contributes trusted include rules, a managed-worktree marker, and refusal to delete a tree whose cleanliness cannot be proved. T3 Code contributes intent recorded before a side effect, and a provider adapter. Ghostex contributes opening any worker directly. Paseo contributes opaque project and workspace ids, and the split between `cwd` and worktree root. Their daemons, event-sourced engines, and cloud runtimes are not copied.

## 4. Domain

```text
Project prj_<ulid>
  slug, lifecycle, availability
  primary session
  PROJECT.md, MEMORY.md, TASKS.md

  Repository repo_<ulid>     projection of PROJECT.md repos
  Workspace wks_<ulid>
    cwd, worktree_root, branch, base
    environment_id
    ownership legacy | managed
    availability
  Thread thr_<ulid>
    label t-0007
    lifecycle
  Session
    environment_id, socket, pane_id, terminal_id
```

Ids come from the `ulid` crate. They are not derived from names or paths.

Lifecycle is `active | paused | archived | deleted`. Availability is `available | missing | invalid`. A missing directory does not change lifecycle.

Thread lifecycle is `planned | provisioning | running | completed | resolved | archived | failed`. Milestone 1 maps today's `starting | open | failed | resolved` into that set when it saves a row. Attention and review stay computed. They are not lifecycle states.

`projects.primary_session_id` is the only automatic route. A stale primary is reported. Another agent in the directory is not selected.

`coordinator adopt` binds one named pane. `HERDR_PANE_ID` is that pane when the command runs inside Herdr. Otherwise `--pane` is required. A live primary is not replaced unless `--replace-primary` is set.

Herdr pane ids are session-local (`w1:p1` in `docs/herdr-notes.md`). A live binding is unique on environment, socket, and pane id. `terminal_id` detects reuse of that triple. The same pane id on another socket or environment is a different binding.

The local environment id is `env_local`. A remote environment is keyed by Herdr's machine `id`. A legacy label that cannot be resolved becomes an environment with `state = unresolved`. The project still imports. Work that must run on that machine refuses.

## 5. Relationships

A thread's workspace, a workspace's repository, and a project's primary session must belong to the same project. Composite foreign keys enforce that. `ON DELETE` is `RESTRICT`.

Two projects may record the same primary checkout `cwd`. They may not own the same managed `worktree_root` on one environment.

## 6. Persistence

The registry is `<projects-root>/registry.sqlite`, opened with `rusqlite` and the bundled SQLite. WAL is set on the file. Every connection sets `foreign_keys = ON` and `busy_timeout = 5000`. Schema version is 2. `projects.pending_slug` reserves a rename target.

`PROJECT.md` remains the writer for name, goal, instructions, profile names, limits, and repo membership. SQLite does not store a second writable copy of those. Repository rows are a projection. `context`, `thread start`, `open`, and `doctor` reconcile when a hash of the `repos` table changes.

After import, this program does not read `.state/project.json`, `.state/coordinator.json`, or `threads/*.toml` to decide behavior, and it does not write them. `doctor` says that binary 0.2.34 must not be used against a migrated root.

Executable setup, include rules, and cleanup policy stay in the user config. The coordinator cannot write them. Milestone 2 implements them. A repository file never raises the user's safety policy.

Configuration precedence is project, then user, then built-in, and only for keys the project may set.

## 7. Worktrees

Not in Milestone 1. Managed worktrees will be created with `git worktree add` at a reserved path, marked, reconciled, given allowed files and ports, set up, then opened with `herdr worktree open`. `herdr worktree create` stays the path for legacy rows because it chooses the directory.

## 8. Profiles

Milestone 3 moves harness flag tables behind one adapter. The coordinator still picks a profile name from the allow-list. Changing the profile does not change the project id.

## 9. Coordinator

`open` focuses the primary pane when it is alive and the profile matches. Otherwise it starts one agent and sets `primary_session_id`. `open --new` does not change the primary. The folder is the location. The session row is ownership.

## 10. TUI

Milestone 1 does not change the popup. Milestone 5 adds the project list and dashboard. Ratatui is for that surface, not for this milestone.

## 11. Configuration

See section 6.

## 12. Archive and delete

Archive sets lifecycle to `archived` and keeps files and rows. Delete moves the directory to `.trash/` and sets lifecycle to `deleted`. It does not remove a worktree this program cannot prove it owns.

## 13. Recovery

`create_project`, `rename_project`, and `delete_project` record an operation before the filesystem effect. `open_coordinator` records its intent, including whether a new workspace is required, before Herdr creates a workspace, a tab, or an agent. A retry reuses that workspace and tab. `coordinator adopt` records the session with the environment id of the named machine. Archive is one SQLite transaction and does not move files, so it has no filesystem recovery step. A pending create with no directory resumes. A done project whose directory disappears becomes `missing` and is not recreated. A session whose terminal id changed is stale. Routing does not select another pane.

## 14. Authority

The coordinator delegates. It does not invent launch flags or setup commands. Events store `actor_kind` and `actor_id`, not a label string. They do not store secrets or raw terminal text. Cross-project writes fail in SQLite.

## 15. Migration

Startup applies the schema, then imports directories that are not yet in `legacy_imports`. A malformed `PROJECT.md` stays `failed` and is retried. A missing machine does not fail the project import.

## 16. Milestones

Milestone 1 is the registry, the primary session, adopt, and the proof below. It is not managed worktrees and it is not the TUI. Later milestones are the workspace service, provider adapters, stored attention and review, the TUI, the compact digest, subscriptions, and hardening.

## Verification

`cargo test` is necessary and is not sufficient. Milestone 1 is accepted only when the evidence shows SQLite is the operational store, poisoned legacy files do not change behavior, legacy operational files are not rewritten, import preserves field meaning, recovery follows the operation row, concurrent processes do not duplicate or cross-wire state, routing uses the primary session, pane reuse rebinds on `terminal_id`, repository edits reconcile before repo-dependent work, and an unresolved machine degrades only that environment.

Proof is real SQLite constraint tests, hand-written 0.2.34 fixtures, a poison test, a new store instance after restart, process-level races, CLI subprocess checks, a boundary check in CI, and a disposable Herdr run. `.github/workflows/release.yml` only builds release binaries. `.github/workflows/ci.yml` runs format, clippy, and `cargo test --locked` on Linux with Rust 1.89. Linux stable and macOS stable run `cargo test --locked` only. Legacy operational files are parsed only in `src/legacy_import.rs`.

Milestone 1 stops when that evidence is reported. Milestone 2 does not start in the same batch.
