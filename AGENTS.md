# Repository Guidelines

## Project Structure & Module Organization

- `src/` contains the Rust bridge implementation. `main.rs` owns startup and wiring; `codex.rs` speaks JSON-RPC to the Codex app-server; `discord.rs` handles Serenity, commands, and interaction UI; `state.rs` stores channel/thread mappings; `config.rs`, `config_ext.rs`, and `options.rs` manage configuration and command options.
- `tests/` contains integration and regression tests, usually named `*_tests.rs`.
- `data/` holds generated runtime state such as `state.json`. Do not commit runtime state or edit it manually for normal changes.
- `target/` contains build artifacts.
- Use `.env.example` and `bridge.json.example` as the canonical configuration templates.

## Build, Test, and Development Commands

- `cargo fmt --all`: format all Rust code.
- `cargo clippy --all-targets -- -D warnings`: lint and treat warnings as failures.
- `cargo test`: run all tests (currently 72: 14 unit + 58 integration).
- `cargo check`: quickly verify compilation.
- `cargo build --release`: build the production binary.
- `cargo run --release`: run the bridge with the built release profile.

## Testing Policy (STRICT)

These rules exist because three bugs shipped despite a large test suite: the tests asserted against copies of the logic instead of the real code. Do not regress this.

### Test the real code, never a copy

- If a test needs a pure function, **extract that function into the module the binary uses** (`src/options.rs` is the established home) and test it there.
- **Forbidden:** writing a `fn route(...)` inside a test file that mirrors logic from `src/main.rs` or `src/discord.rs`. That tests your assumption, not the code. This exact failure caused the subcommand-options bug.
- When protocol shapes matter (serenity types, JSON-RPC payloads), build fixtures by deserializing into the **real types** and verify the wire shape against the dependency's source (e.g. `RawCommandDataOption` in serenity), not against a hand-written guess.

### Regression test for every bug

- Every production bug fix must come with a test that **fails against the old code and passes against the new**. Name the test after the failure mode (e.g. `empty_string_name_is_treated_as_missing` for the `Some("")` thread-name bug).
- If you cannot write such a test, say so explicitly in the commit message and explain why.

### Pure-logic extraction rule

- Decision logic (routing, naming, gating, payload shapes) belongs in pure functions that take data in and return data out. Async I/O and side effects stay in thin wrappers.
- A pure function is untestable only if it doesn't exist. If you find logic inside an async handler that a test should cover, extract it before writing the test.

### Before every push

- `cargo fmt --all`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test` — full suite green, no ignored tests added.
- If you edited code by string replacement or line surgery, **read the modified region afterward** to confirm the file is intact. Several patches corrupted adjacent code silently.

### Anti-patterns that caused shipped bugs

1. **Copy-tests:** `tests/steer_tests.rs` originally duplicated routing logic instead of testing `state.rs`. Fixed by extraction; do not repeat.
2. **Fixture guessing:** deserializing serenity options without `type`/`options` fields failed at runtime, not at test time. Fixtures must be validated against the dependency source.
3. **Unverified string patches:** a `Replace()` with CRLF mismatch silently no-oped, shipping a "fix" that wasn't in the binary. After any patch, `git grep` for the new code to confirm it exists.

## Coding Style & Naming Conventions

- Rust 2024 edition; follow `rustfmt` output exactly.
- Use `snake_case` for functions/modules/variables, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants.
- Prefer Tokio async idioms; avoid blocking synchronization inside async tasks. **Never** call `blocking_lock()` / `block_on()` on a tokio runtime thread — use `std::sync::Mutex` for short, non-await locks (see `state.rs` `default_model`).
- Keep Codex protocol handling separate from Discord presentation logic.
- Use explicit Serde attributes where configuration keys or protocol payloads differ from Rust field naming. **Note:** `BridgeConfig` needs `rename_all = "camelCase"` — the file uses `autoThread`, the field is `auto_thread`. A missing rename silently yields `Default` values.

## Commit & Pull Request Guidelines

- Git history uses Conventional Commit prefixes: `feat:`, `fix:`, and `test:`.
- Write concise, imperative summaries, for example: `fix: restore autoThread branch in free-form chat`.
- Pull requests should describe motivation, behavior changes, tests performed, configuration/protocol impact, and include Discord screenshots or relevant log excerpts for UI changes.

## Security & Configuration Tips

- Never commit `.env`, real `bridge.json`, bot tokens, or Discord IDs.
- Copy example files locally and rotate any token that may have leaked.
- Preserve the controller-user authorization boundary and validate attachment assumptions.