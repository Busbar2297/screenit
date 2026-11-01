# Development

Toolchain: rustup stable-msvc, VS Build Tools + Windows SDK. Windows 10 1903+ for capture APIs.

```sh
cargo build --workspace
cargo test --workspace                 # fast tests, no desktop needed
cargo xtask ci                         # fmt, clippy, tests, layers, parity, scorecard, panic-hunt
cargo xtask ci --e2e                   # adds capture/recording e2e against the fixture window
cargo xtask bless                      # regenerate golden images (tests/golden)
cargo run -p screenit-cli -- commands
```

## Tests

- Unit and golden tests: `testkit::assert_image_eq(actual, "name", tolerance)`; goldens live in
  `crates/<crate>/tests/golden/`. `SCREENIT_BLESS=1` rewrites them.
- e2e (feature `e2e`, `#[ignore]` by default): launch `screenit-fixture`, capture it, assert pixels.
- `panic-hunt`: fuzzes every registered command with adversarial params; must return `Err`.

## Fixture window

`screenit-fixture` paints deterministic patterns (checkerboard, colour bars, numbered rows) from a
seed, can be taller than the screen to exercise scrolling, and exposes a UIA tree.
