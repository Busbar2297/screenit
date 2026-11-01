# AGENTS.md: guide for AI agents and contributors

screenIt is a Windows-only, pure-Rust screen capture, recording and annotation tool whose goal is
feature parity with the well-known commercial capture tools (catalogue: `parity/snagit.toml`),
built engine-first.
Product name is always **screenIt** in user-facing text; machine names are lowercase
(`screenit-*` crates, `screenit` binary). Rust only: no JavaScript, no webview, no Electron/Tauri.
No ffmpeg: video goes through Media Foundation inside `screenit-platform`.

## 1. Orientation

| Read | Why |
|---|---|
| `docs/architecture.md` | crate map, layers, engine/UI seam |
| `docs/development.md` | build, test, run, drive the app, e2e |
| `docs/control-protocol.md` | JSON control channel used by agents and tests |
| `docs/gaps.md` | ranked work list |
| `docs/parity-checklist.md` | GENERATED from `parity/snagit.toml` (`cargo xtask parity`) |
| `docs/scorecard.md` | GENERATED measured numbers (`cargo xtask scorecard`) |
| `log/devlog.md` | what the previous session did, what is still open |

## 2. Workspace map and layers

```text
L0 geom raster codecs        foundation, no workspace deps above L0
L1 doc                       capture document model (pure data)
L2 ops algo vector           history, imaging algorithms, vector geometry
L3 compose format            CPU compositor (the oracle), native .scit format
L4 capture record library    capture/recording models, library index; define the platform TRAITS
L5 platform engine           platform = the ONE crate allowed unsafe (Win32/WGC/MF/WASAPI/UIA);
                             engine = Session + command registry; engine never depends on platform
L6 ui-egui automation        egui shell, MCP server
   testkit                   test helpers + fake platform (may depend on anything below L6)
apps/ screenit | screenit-cli | screenit-fixture (the capture oracle window)
xtask/ layers | ci | parity | scorecard | bless | panic-hunt | perf
```

`cargo xtask layers` enforces it: a crate may depend only on equal-or-lower layers per the table in
`xtask/src/layers.rs`. Register new crates there.

## 3. Golden rules

1. **Never crash.** Non-test code has no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`,
   `unreachable!`. Return the crate error and use `?`. Input-derived numbers are hostile: use
   `get()`, `checked_*`, `saturating_*`, guard NaN/zero division, cap allocations. Every crash fix
   ships with a regression test.
2. **No `unsafe`** (workspace `forbid`) except `screenit-platform` (`deny` crate-wide, each block
   under `#[allow(unsafe_code)]` with a `// SAFETY:` comment).
3. **Everything is a command.** User-visible behaviour is a `CommandSpec` in
   `crates/engine/src/*_cmds.rs` registered in `commands.rs`: id, label, menu path, shortcut,
   params doc, `enabled`, `run`. UI, CLI, control channel and MCP dispatch by id.
4. **Platform behind traits.** OS access goes through traits defined in `capture`/`record`
   (`ScreenSource`, `WindowSource`, `Recorder`, ...), implemented by `platform`, faked in `testkit`.
5. **The UI is thin and data-driven.** State is serde so the control channel can read/drive it.
6. **Tests are the gate.** Pixel code gets golden/oracle tests; capture code gets e2e tests against
   `screenit-fixture`. Never lower the parity `FLOOR`.
7. **Verify UI visually.** Screenshot the real window (PrintWindow works even when it is not in
   front) and look at the PNG; drive state through the control channel.
8. **Performance is a feature.** Budgets live in `perf/budgets.toml`.
9. **Clean-room.** Snagit is studied for behaviour and public documentation only. No proprietary
   code, assets or stamps. Every bundled asset is permissively licensed and listed in `ATTRIBUTION.md`.

## 4. Picking work

1. `docs/gaps.md` and `docs/scorecard.md`.
2. `cargo xtask parity`: lowest-tier `missing` / `partial` rows in `parity/snagit.toml`.
3. `log/devlog.md` "Still open" bullets.

## 5. Before you finish a task

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask layers
cargo xtask parity          # if you added commands or flipped a parity row; commit docs/parity-checklist.md
cargo xtask scorecard       # commit docs/scorecard.md
cargo xtask panic-hunt      # if you added/changed commands
cargo xtask ci --e2e        # if you touched capture/record/platform (needs a desktop session)
```

Then append a terse entry to `log/devlog.md`. The tree must build at every step.

## 6. Adding a command

1. Pick an id (`capture.region`, `editor.arrow.add`...) and add/flip the row in `parity/snagit.toml`.
2. Algorithm in the lowest crate that fits (`algo`, `vector`, `capture`, `record`).
3. Command in `crates/engine/src/<area>_cmds.rs` with `specs()`; register in `commands.rs`.
4. Exactly one history step per command (use `coalesce` for drags/typing).
5. Tests: behaviour, undo/redo, disabled states, bad params (must `Err`, not panic).
6. `cargo xtask parity` and commit the regenerated checklist.
