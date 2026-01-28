# AGENTS.md

## Build, Lint, and Test Commands

This project is a Rust application using Cargo as the build system.

### Build Commands
```bash
# Build the project in debug mode
cargo build

# Build the project in release mode (optimized)
cargo build --release

# Build and run in one step
cargo run

# Build and run in release mode
cargo run --release 2>&1
```

### Test Commands
```bash
# Run all tests (if any exist)
cargo test

# Run a single test by name
cargo test <test_name>

# Run tests with verbose output
cargo test --verbose

# Run tests in release mode
cargo test --release
```

### Code Quality Tools
```bash
# Check code formatting
cargo fmt

# Check for common issues
cargo clippy

# Check code formatting and clippy in one command
cargo check
```

## Code Style Guidelines

### General Coding Standards
- Follow Rust idioms and best practices
- Use descriptive variable and function names
- Prefer explicit type annotations for public APIs
- Keep functions focused and small (single responsibility principle)
- Use `snake_case` for variables, functions, and modules
- Use `PascalCase` for types and enums

### Imports and Module Structure
- Group imports alphabetically within sections
- Separate standard library, external crates, and local modules with blank lines
- Use `use` statements at the top of files for commonly used items
- Organize modules in a logical file structure under `src/`

### Formatting
- Use `cargo fmt` for code formatting (Rustfmt)
- Maximum line length of 100 characters
- Consistent indentation with 4 spaces (not tabs)
- No trailing whitespace
- Newline at end of file

### Types and Naming Conventions
- Use `snake_case` for variables and functions
- Use `PascalCase` for types (structs, enums, traits)
- Use `SCREAMING_SNAKE_CASE` for constants
- Prefix private functions with underscore if they are not used externally

### Error Handling
- Prefer using `?` operator for error propagation
- Use `Result<T, E>` for operations that can fail
- Return `Option<T>` for potentially missing values
- Avoid `unwrap()` and `expect()` in production code
- Use `panic!` only for unrecoverable errors

### Documentation
- Document public APIs with Rustdoc comments using `///`
- Use `//!` for module-level documentation
- Include examples where appropriate
- Document non-obvious behavior or complex logic

### Conventions for This Specific Project

#### Configuration Module
- All configuration values are defined in the `config` module
- Constants use `SCREAMING_SNAKE_CASE`
- Configuration parameters are documented with comments

#### Performance-Critical Code
- Use `#[inline(always)]` for performance-critical functions
- Prefer `Vec` over other collections when appropriate
- Use `rayon` for parallel processing where applicable
- Minimize allocations in hot paths

#### Logging and Output
- Use `println!` for user-facing output
- Use `writeln!` with file writers for logs
- Log messages include thread identification where relevant
- All output files are written to timestamped directories

#### File Structure
- Input files should be named: `input-fixed.txt`, `input-roots.txt`, `input-division.txt`
- Output files are organized in timestamped directories with:
  - `output-字根.txt` (root key assignments)
  - `output-编码.txt` (character codes)
  - `log.txt` (thread logs)
  - `总结.txt` (summary file)

#### Code Comments
- Use Chinese comments for internal documentation as the project is primarily in Chinese
- Provide detailed explanations of complex algorithms
- Document algorithmic parameters and their impact on performance

## Cursor/Copilot Rules

This repository does not contain any `.cursor/rules/` or `.github/copilot-instructions.md` files.