# dwm-status-bar-rs

A simple, performant, and suckless-style status bar for `dwm` written in Rust.

`cpu: 7% | ram: 42% | disk: 58% | cpu: 45°C | gpu: 51°C | bat: 88% D | vol: 75% | net:  up | Mon 27 May 14:30:15`

## About

This project is heavily inspired by [babashka-status-bar](https://github.com/perpen/babashka-status-bar). The goal was to create a similarly simple and modular status bar. While Babashka is excellent, using Rust with Tokio provides a highly performant, single-binary solution with the safety and expressiveness of Rust's type system.

## Features

*   **Asynchronous:** Built with `tokio` to handle updates concurrently without blocking.
*   **Modular:** Easily add or remove status components.
*   **Performant:** Low CPU and memory footprint.
*   **Manual Triggers:** Update individual modules on demand via filesystem events.
*   **Conditional:** Modules are only enabled if their dependencies (e.g., system files or commands) are found.
*   **Dynamic Hiding:** A module can return an empty string to temporarily hide itself from the bar.

## Installation

### Prerequisites

*   Rust toolchain (`cargo`)
*   `xsetroot` (usually installed with Xorg)
*   Dependencies for optional modules you wish to use (e.g., `acpi`, `amixer`).

### Building

Clone the repository and build the release binary:

```sh
git clone <repository-url>
cd dwm-status-bar-rs
cargo build --release
```

### Running

Add the compiled binary to your `~/.xinitrc` or dwm startup script:

```sh
# ~/.xinitrc
/path/to/target/release/dwm-status-bar-rs &
```

## Customization

Adding a new module is straightforward:

#### 1. Write the monitor function

Create an `async fn` that returns a `Result<String>`. The string will be displayed in the bar.

```rust
async fn weather_monitor() -> Result<String> {
    // This could be an API call or a command
    let output = run_command("curl", &["-s", "wttr.in?format=%C+%t"]).await?;
    Ok(format!("weather: {}", output))
}
```

#### 2. Add the module to `MODULE_ORDER`

Add your module's ID to the array in `main.rs`. The order here dictates the display order.

```rust
const MODULE_ORDER: &[&str] = &[
    "cpu_load", "ram", "disk", "weather", /* ... */
];
```

#### 3. Spawn the monitor in `main`

Add a `spawn_monitor` call in the `main` function.

```rust
// In main()
spawn_monitor("weather", Duration::from_secs(900), weather_monitor, update_tx.clone(), trigger_sub(), args.profile);
```

## Manual Triggers

You can force an immediate update for any module by creating a file in `/tmp/dwm-bar-triggers/` with the name of the module's ID. This is useful for keybindings.

For example, to refresh the volume module:

```sh
touch /tmp/dwm-bar-triggers/volume
```
