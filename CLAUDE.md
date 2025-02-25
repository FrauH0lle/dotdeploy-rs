# dotdeploy-rs Development Guide

## Build Commands
- Build: `cargo build`
- Run: `cargo run`
- Release build: `cargo build --release`
- Test: `cargo test`
- Run single test: `cargo test test_name -- --exact`
- Check: `cargo check`
- Clippy: `cargo clippy -- -D warnings`
- Format: `cargo fmt`

## Code Style Guidelines
- **Imports**: Group by standard lib, external crates, then internal modules
- **Error Handling**: Use `color-eyre` for errors; wrap with context using `wrap_err()`
- **Naming**: Snake case for variables/functions, Pascal for types, SCREAMING_CASE for constants
- **Documentation**: Follow conventions in `conventions.org` - brief description, details, param docs, errors section
- **Async**: Use Tokio runtime for async operations
- **Logging**: Use tracing macros (`debug!`, `info!`, etc.) with structured logging
- **Testing**: Include meaningful assert messages in tests
- **Types**: Prefer strong typing with appropriate use of Rust type system

## Modules Organization
- Core modules: cli, config, store, logs, utils
- SQLite backend for storage in store/sqlite_* modules