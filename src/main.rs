/*
How to add a new module to the status bar:

1.  Write the monitor function:
    - Create a new `async fn your_monitor_name() -> Result<String>`.
    - This function should perform the check and return a `Result` containing the formatted string to display.
    - For performance, use `tokio::process::Command` for external commands instead of `std::process::Command`.
    - See `battery_monitor` or `volume_monitor` for examples.

2.  Add the module to `MODULE_ORDER`:
    - Add a unique string ID for your module to the `MODULE_ORDER` constant array. The order in this array determines the display order in the bar.
    - Example: `const MODULE_ORDER: &[&str] = &["..., "your_module_id"];`

3.  Spawn the monitor in `main`:
    - In the `main` function, add a `spawn_monitor` call for your new module.
    - Provide the ID, a `Duration` for the update interval, the function name, and the channels.

4.  (Optional) Add a manual trigger:
    - If you want to be able to manually trigger an update (e.g., via a script or keybinding), your monitor will automatically support it.
    - Simply create an empty file in `/tmp/dwm-bar-triggers/` with the same name as your module ID.
*/
use anyhow::Result;
use clap::Parser;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::env;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{CpuExt, DiskExt, System, SystemExt};
use tokio::sync::{broadcast, mpsc};

const MODULE_ORDER: &[&str] = &[
   "vpn", "notification", "cpu_load", "ram", "disk", "cpu_temp", "gpu_temp", "battery", "volume", "bluetooth", "net", "datetime",
];
const TRIGGER_DIR: &str = "/tmp/dwm-bar-triggers";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Enable profiling mode to measure module execution time.
    #[arg(short, long)]
    profile: bool,
}

#[derive(Debug, Clone)]
struct Update {
    id: &'static str,
    value: String,
}

fn command_exists(cmd: &str) -> bool {
    if let Ok(path_var) = env::var("PATH") {
        for path in path_var.split(':') {
            if Path::new(&format!("{}/{}", path, cmd)).exists() {
                return true;
            }
        }
    }
    false
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    tracing_subscriber::fmt::init();
    fs::create_dir_all(TRIGGER_DIR).expect("Cannot create trigger directory");

    let (update_tx, mut update_rx) = mpsc::channel::<Update>(32);
    let (trigger_tx, _) = broadcast::channel::<&'static str>(16);
    let results = Arc::new(Mutex::new(HashMap::new()));
    let sys = Arc::new(Mutex::new(System::new_all()));

    let trigger_sub = || trigger_tx.subscribe();

    // --- Core modules (always enabled) ---
    spawn_monitor("datetime", Duration::from_secs(1), datetime_monitor, update_tx.clone(), trigger_sub(), args.profile);
    let sys_clone = sys.clone();
    spawn_monitor("disk", Duration::from_secs(30), move || disk_monitor(sys_clone.clone()), update_tx.clone(), trigger_sub(), args.profile);
    let sys_clone = sys.clone();
    spawn_monitor("ram", Duration::from_secs(5), move || ram_monitor(sys_clone.clone()), update_tx.clone(), trigger_sub(), args.profile);
    spawn_monitor("cpu_load", Duration::from_secs(2), cpu_load_monitor, update_tx.clone(), trigger_sub(), args.profile);
    spawn_monitor("vpn", Duration::from_secs(10), vpn_monitor, update_tx.clone(), trigger_sub(), args.profile);

    // --- Conditional modules (check for dependencies) ---
    if Path::new("/sys/class/thermal/thermal_zone0/temp").exists() {
        spawn_monitor("cpu_temp", Duration::from_secs(10), cpu_temp_monitor, update_tx.clone(), trigger_sub(), args.profile);
    }
    if Path::new("/sys/class/thermal/thermal_zone1/temp").exists() {
        spawn_monitor("gpu_temp", Duration::from_secs(30), gpu_temp_monitor, update_tx.clone(), trigger_sub(), args.profile);
    }
    if Path::new("/home/sky/nix-config/bash/network-status.sh").exists() {
        spawn_monitor("net", Duration::from_secs(10), network_monitor, update_tx.clone(), trigger_sub(), args.profile);
    }
    if command_exists("acpi") {
        spawn_monitor("battery", Duration::from_secs(30), battery_monitor, update_tx.clone(), trigger_sub(), args.profile);
    }
    if command_exists("amixer") {
        spawn_monitor("volume", Duration::from_secs(10), volume_monitor, update_tx.clone(), trigger_sub(), args.profile);
    }
    if command_exists("bluetoothctl") {
        spawn_monitor("bluetooth", Duration::from_secs(60), bluetooth_monitor, update_tx.clone(), trigger_sub(), args.profile);
    }
    if command_exists("dunst") {
        spawn_monitor("notification", Duration::from_secs(600), notification_monitor, update_tx.clone(), trigger_sub(), args.profile);
    }

    tokio::spawn(trigger_listener(trigger_tx));

    while let Some(update) = update_rx.recv().await {
        let mut results_guard = results.lock().unwrap();
        results_guard.insert(update.id, update.value);
        let bar_string = assemble_bar(&results_guard);
        drop(results_guard);
        set_xroot_name(&bar_string);
    }
}

fn spawn_monitor<F, Fut>(
    id: &'static str,
    interval_duration: Duration,
    monitor_fn: F,
    tx: mpsc::Sender<Update>,
    mut trigger_rx: broadcast::Receiver<&'static str>,
    profile: bool,
) where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<String>> + Send,
{
    let monitor_fn = Arc::new(monitor_fn);

    let task = move || {
        let monitor_fn = Arc::clone(&monitor_fn);
        async move {
            let start = Instant::now();
            let result = monitor_fn().await;
            if profile {
                tracing::info!(module = id, duration_ms = start.elapsed().as_millis(), "Module executed");
            }
            result
        }
    };

    tokio::spawn(async move {
        match task().await {
            Ok(value) => {
                if tx.send(Update { id, value }).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::warn!("Disabling monitor '{}' because initial run failed: {}", id, e);
                return;
            }
        }
        let mut interval = tokio::time::interval(interval_duration);
        loop {
            tokio::select! {
                _ = interval.tick() => {},
                Ok(triggered_id) = trigger_rx.recv() => {
                    if triggered_id != id { continue; }
                    tracing::info!("Triggered update for {}", id);
                }
            }
            match task().await {
                Ok(value) => {
                    if tx.send(Update { id, value }).await.is_err() {
                        break;
                    }
                }
                Err(e) => tracing::error!("Monitor '{}' failed: {}", id, e),
            }
        }
    });
}

async fn trigger_listener(tx: broadcast::Sender<&'static str>) -> Result<()> {
    use notify::{Error, RecursiveMode};
    use notify_debouncer_mini::{new_debouncer, DebouncedEvent};

    let mut debouncer = new_debouncer(Duration::from_millis(100), move |res: Result<Vec<DebouncedEvent>, Error>| {
        if let Ok(events) = res {
            for event in events {
                if let Some(id_str) = event.path.file_name().and_then(|s| s.to_str()) {
                    if let Some(id) = MODULE_ORDER.iter().find(|&&m| m == id_str) {
                        let _ = tx.send(id);
                    }
                }
            }
        }
    })?;
    debouncer.watcher().watch(Path::new(TRIGGER_DIR), RecursiveMode::NonRecursive)?;
    std::future::pending::<()>().await;
    Ok(())
}

fn assemble_bar(results: &HashMap<&'static str, String>) -> String {
    let parts: Vec<String> = MODULE_ORDER
        .iter()
        .filter_map(|&id| results.get(id).cloned().filter(|s| !s.is_empty()))
        .collect();
    format!(" {} ", parts.join(" | "))
}

fn set_xroot_name(name: &str) {
    if let Err(e) = Command::new("xsetroot").arg("-name").arg(name).status() {
        tracing::error!("Failed to run xsetroot: {}", e);
    }
}

async fn run_command(cmd: &str, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new(cmd).args(args).output().await?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    } else {
        anyhow::bail!("Command '{}' failed: {}", cmd, String::from_utf8_lossy(&output.stderr))
    }
}

// --- Individual Monitor Functions ---

async fn datetime_monitor() -> Result<String> {
    Ok(chrono::Local::now().format("%a %d %b %H:%M:%S").to_string())
}

async fn disk_monitor(sys: Arc<Mutex<System>>) -> Result<String> {
    let mut sys = sys.lock().unwrap();
    sys.refresh_disks_list();
    let root_disk = sys.disks().iter().find(|d| d.mount_point() == Path::new("/")).ok_or_else(|| anyhow::anyhow!("'/' disk not found"))?;
    let used_pct = (root_disk.total_space() - root_disk.available_space()) as f64 * 100.0 / root_disk.total_space() as f64;
    Ok(format!("disk: {:.0}%", used_pct))
}

async fn ram_monitor(sys: Arc<Mutex<System>>) -> Result<String> {
    let mut sys = sys.lock().unwrap();
    sys.refresh_memory();
    let used_pct = sys.used_memory() as f64 * 100.0 / sys.total_memory() as f64;
    Ok(format!("ram: {:.0}%", used_pct))
}

async fn read_temp(path: &str) -> Result<String> {
    let temp_str = fs::read_to_string(path)?;
    let temp = temp_str.trim().parse::<f32>()? / 1000.0;
    Ok(format!("{:.0}°C", temp))
}

async fn cpu_temp_monitor() -> Result<String> {
    read_temp("/sys/class/thermal/thermal_zone0/temp").await.map(|t| format!("cpu: {}", t))
}
async fn gpu_temp_monitor() -> Result<String> {
    read_temp("/sys/class/thermal/thermal_zone1/temp").await.map(|t| format!("gpu: {}", t))
}

async fn network_monitor() -> Result<String> {
    run_command("/home/sky/nix-config/bash/network-status.sh", &[]).await
}

async fn vpn_monitor() -> Result<String> {
    if Path::new("/sys/class/net/tun0").exists() {
        Ok("VPN".to_string())
    } else {
        Ok(String::new())  // Empty string = hidden from bar
    }
}

async fn cpu_load_monitor() -> Result<String> {
    let mut sys = System::new();
    sys.refresh_cpu();
    tokio::time::sleep(System::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_cpu();
    let usage = sys.global_cpu_info().cpu_usage();
    Ok(format!("cpu: {:.0}%", usage))
}

async fn battery_monitor() -> Result<String> {
    // Requires `acpi` to be installed
    let acpi_output = run_command("acpi", &["-b"]).await?;
    let charge_threshold_output = run_command("cat", &["/sys/class/power_supply/BAT0/charge_stop_threshold"]).await?;

    let re = Regex::new(r"Battery 0: ([\w\s]+), (\d+)%")?;
    if let Some(caps) = re.captures(&acpi_output) {
        let status = &caps[1];
        let percent = &caps[2];
        let status_char = match status {
            "Charging" => "C",
            "Discharging" => "D",
            "Full" => "F",
            _ => "?",
        };
        Ok(format!("bat: {}/{}% {}", percent, charge_threshold_output, status_char))
    } else {
        Ok("bat: N/A".to_string())
    }
}

async fn bluetooth_monitor() -> Result<String> {
    // Requires `bluetoothctl`
    let cmd = r#"
        CONNECTED_MAC=$(bluetoothctl devices Connected | cut -d' ' -f2)
        if [ -n "$CONNECTED_MAC" ]; then
            INFO=$(bluetoothctl info $CONNECTED_MAC)
            NAME=$(echo "$INFO" | grep "Name:" | cut -d' ' -f2-)
            BATTERY=$(echo "$INFO" | grep "Battery Percentage" | sed -n 's/.*(\(.*\))/\1/p')
            if [ -n "$BATTERY" ]; then
                echo "bt: $NAME ${BATTERY}%"
            else
                echo "bt: $NAME"
            fi
        fi
    "#;
    run_command("bash", &["-c", cmd]).await
}

async fn volume_monitor() -> Result<String> {
    // Requires `amixer` from alsa-utils
    let cmd = "amixer sget Master | awk -F'[][]' '/Front Left:/ { print $2 }'";
    let volume = run_command("bash", &["-c", cmd]).await?;
    Ok(format!("vol: {}", volume))
}

async fn notification_monitor() -> Result<String> {
    let is_paused = run_command("dunstctl", &["is-paused"]).await?;
    if is_paused.trim() == "true" {
        Ok("n: disabled".to_string())
    } else {
        Ok(String::new())
    }
}
