# AGENTS.md

## Build Commands

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run
cargo run

# Run release
cargo run --release 2>&1
```

## Test Commands

```bash
cargo test
cargo test --release
```

## Code Quality

```bash
cargo fmt
cargo clippy
cargo check
```

## Code Style Guidelines

### Rust Idioms
- Use `snake_case` for variables/functions, `PascalCase` for types
- Use `SCREAMING_SNAKE_CASE` for constants
- Keep functions focused with single responsibility
- Prefer `?` for error propagation, `Result<T, E>` for fallible operations

### Performance-Critical Code
- Use `#[inline(always)]` for hot-path functions
- Use `rayon` for parallel iteration
- Minimize allocations in inner loops
- Prefer `Vec` over other collections

### Documentation
- Use Chinese comments (项目主要为中文)
- Document complex algorithm logic
- Explain configuration parameters and their impact

## Conventions for This Project

### Key Mappings
- a-z: 0-25
- _: 26
- ;: 27, ,: 28, .: 29, /: 30

### Data Structures
- `MAX_CODE_VAL = 31^3 = 29791` - total possible codes
- `EQUIV_TABLE_SIZE = 31` - key pairs including terminator

### Modules
- `config` - All configuration constants
- High-performance data structures for precomputation
- `Evaluator` - Incremental update state machine
- File loaders with format validation
