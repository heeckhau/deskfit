use std::time::Duration;

use anyhow::{Result, bail};
use btleplug::api::CentralState;
use clap::{Parser, Subcommand};
use deskfit::ble::{self, Treadmill};
use deskfit::config::{self, Config};
use deskfit::protocol::{Command as Packet, Speed, Status};

const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const FIND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Parser)]
#[command(version, about = "Control a DeskFit treadmill over Bluetooth")]
struct Cli {
    /// Re-launch as its own responsible process before running the command,
    /// so macOS checks this executable's Bluetooth permission rather than
    /// the launching app's.
    #[arg(long, global = true, hide = true)]
    disclaim: bool,

    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// Also print each status packet's raw bytes.
    #[arg(long, global = true, hide = true)]
    raw: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List nearby Bluetooth devices, treadmill candidates first.
    Scan {
        /// Scan duration in seconds.
        #[arg(long, default_value_t = 5)]
        seconds: u64,
    },
    /// Choose the treadmill to control and remember it.
    Setup {
        /// Peripheral identifier from `deskfit scan`. Required when more than
        /// one candidate is nearby.
        #[arg(long)]
        id: Option<String>,
    },
    /// Print the current treadmill status once.
    Status,
    /// Print the treadmill status every half second until interrupted.
    Watch,
    /// Start the belt, or resume a paused session.
    Start(Observe),
    /// Pause the belt.
    Pause(Observe),
    /// Stop the belt and end the session.
    Stop(Observe),
    /// Set the target speed in tenths of km/h (10 = 1.0 km/h, 80 = 8.0 km/h).
    Speed {
        #[arg(value_parser = parse_speed)]
        value: Speed,
        #[command(flatten)]
        observe: Observe,
    },
    /// Own the treadmill connection and take JSON-line requests on stdin.
    /// Used by the Stream Deck plugin.
    Serve,
    /// Experiment: keep a connection open, polling every `--poll` seconds
    /// (0: only once), and print each status change and connection loss.
    #[command(hide = true)]
    Hold {
        #[arg(long, default_value_t = 0)]
        poll: u64,
    },
}

#[derive(Clone, Copy)]
struct Output {
    json: bool,
    raw: bool,
}

#[derive(clap::Args)]
struct Observe {
    /// Seconds to keep printing status after sending the command.
    #[arg(long, default_value_t = 5)]
    observe: u64,
}

fn parse_speed(s: &str) -> Result<Speed, String> {
    let raw: u8 = s.parse().map_err(|e| format!("{e}"))?;
    Speed::new(raw).ok_or_else(|| format!("must be between {} and {}", Speed::MIN.raw(), Speed::MAX.raw()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    #[cfg(target_os = "macos")]
    if cli.disclaim {
        let args: Vec<_> = std::env::args_os().skip(1).filter(|a| a != "--disclaim").collect();
        let code = deskfit::responsibility::spawn_disclaimed(&args)?;
        std::process::exit(code);
    }

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(run(cli.command, Output { json: cli.json, raw: cli.raw }))
}

async fn run(command: Command, out: Output) -> Result<()> {
    match command {
        Command::Scan { seconds } => scan(seconds, out.json).await,
        Command::Setup { id } => setup(id).await,
        Command::Status => {
            let mut treadmill = connect().await?;
            let status = treadmill.fetch_status(STATUS_TIMEOUT).await;
            let printed = status.and_then(|s| print_status(&treadmill, &s, out));
            treadmill.disconnect().await?;
            printed
        }
        Command::Watch => {
            let mut treadmill = connect().await?;
            let result = tokio::select! {
                r = watch(&mut treadmill, None, out) => r,
                _ = tokio::signal::ctrl_c() => Ok(()),
            };
            treadmill.disconnect().await?;
            result
        }
        Command::Start(o) => send(Packet::Start, o, out).await,
        Command::Pause(o) => send(Packet::Pause, o, out).await,
        Command::Stop(o) => send(Packet::Stop, o, out).await,
        Command::Speed { value, observe } => send(Packet::SetSpeed(value), observe, out).await,
        Command::Serve => deskfit::serve::serve().await,
        Command::Hold { poll } => {
            let mut treadmill = connect().await?;
            let result = tokio::select! {
                r = hold(&mut treadmill, poll) => r,
                _ = tokio::signal::ctrl_c() => Ok(()),
            };
            let _ = treadmill.disconnect().await;
            result
        }
    }
}

/// Print status changes with the time since connecting until the
/// connection drops.
async fn hold(treadmill: &mut Treadmill, poll: u64) -> Result<()> {
    let start = tokio::time::Instant::now();
    let stamp = || {
        let s = start.elapsed().as_secs();
        format!("[{:02}:{:02}:{:02}]", s / 3600, s / 60 % 60, s % 60)
    };
    let mut poll_timer = tokio::time::interval(Duration::from_secs(poll.max(1)));
    let mut link_check = tokio::time::interval(Duration::from_secs(5));
    let mut last = String::new();
    treadmill.send(Packet::FetchStatus).await?;
    println!("{} connected, polling {}", stamp(), if poll == 0 { "once".into() } else { format!("every {poll} s") });
    loop {
        tokio::select! {
            status = treadmill.next_valid_status() => {
                let status = status?;
                // Elapsed time and distance change while running; compare the rest.
                let line = format!("{:?} speed {} raw {}", status.state, status.speed, treadmill.last_packet_hex());
                let key = format!("{:?} {} {:?}", status.state, status.speed, status.countdown);
                if key != last {
                    println!("{} {line}", stamp());
                    last = key;
                }
            }
            _ = poll_timer.tick(), if poll > 0 => treadmill.send(Packet::FetchStatus).await?,
            _ = link_check.tick() => {
                if !treadmill.is_connected().await? {
                    println!("{} disconnected by the treadmill", stamp());
                    return Ok(());
                }
            }
        }
    }
}

async fn scan(seconds: u64, json: bool) -> Result<()> {
    let report = deskfit::scan::scan(Duration::from_secs(seconds)).await?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    println!("Adapter: {}", report.adapter_state);
    for d in &report.devices {
        println!(
            "{} {:<36} {:>5} {}",
            if d.candidate { "*" } else { " " },
            d.id,
            d.rssi.map(|r| r.to_string()).unwrap_or_default(),
            d.name.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

async fn setup(id: Option<String>) -> Result<()> {
    let report = deskfit::scan::scan(Duration::from_secs(5)).await?;
    let device = match id {
        Some(id) => report.devices.into_iter().find(|d| d.id.eq_ignore_ascii_case(&id)),
        None => {
            let candidates: Vec<_> = report.devices.into_iter().filter(|d| d.candidate).collect();
            if candidates.len() > 1 {
                for d in &candidates {
                    eprintln!("  {} {}", d.id, d.name.as_deref().unwrap_or(""));
                }
                bail!("several treadmill candidates found; choose one with --id");
            }
            candidates.into_iter().next()
        }
    };
    let Some(device) = device else {
        bail!("treadmill not found (adapter {}); is it powered on?", report.adapter_state);
    };
    let path = config::save(&Config {
        peripheral_id: device.id.clone(),
        name: device.name.clone(),
    })?;
    println!(
        "Using {} {} (saved to {})",
        device.id,
        device.name.as_deref().unwrap_or(""),
        path.display()
    );
    Ok(())
}

async fn connect() -> Result<Treadmill> {
    let config = config::load()?;
    let (adapter, state) = ble::adapter().await?;
    if state != CentralState::PoweredOn {
        bail!("Bluetooth adapter is {state:?}; check that Bluetooth is on and permitted");
    }
    let peripheral = ble::find(&adapter, &config.peripheral_id, FIND_TIMEOUT).await?;
    Treadmill::connect(peripheral).await
}

/// Send a motor command, then show the reported status so its effect can be
/// confirmed.
async fn send(packet: Packet, observe: Observe, out: Output) -> Result<()> {
    let mut treadmill = connect().await?;
    let before = treadmill.fetch_status(STATUS_TIMEOUT).await;
    let result = async {
        print_status(&treadmill, &before?, out)?;
        treadmill.send(packet).await?;
        if !out.json {
            println!("sent {packet:?}");
        }
        watch(&mut treadmill, Some(Duration::from_secs(observe.observe)), out).await
    }
    .await;
    treadmill.disconnect().await?;
    result
}

/// Poll and print status until `duration` passes, or forever.
async fn watch(treadmill: &mut Treadmill, duration: Option<Duration>, out: Output) -> Result<()> {
    let deadline = duration.map(|d| tokio::time::Instant::now() + d);
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    loop {
        interval.tick().await;
        if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
            return Ok(());
        }
        match treadmill.fetch_status(STATUS_TIMEOUT).await {
            Ok(status) => print_status(treadmill, &status, out)?,
            Err(e) if out.json => println!("{}", serde_json::json!({ "error": e.to_string() })),
            Err(e) => println!("unknown: {e}"),
        }
    }
}

fn print_status(treadmill: &Treadmill, status: &Status, out: Output) -> Result<()> {
    if out.json {
        let mut value = serde_json::to_value(status)?;
        if out.raw {
            value["raw"] = treadmill.last_packet_hex().into();
        }
        println!("{value}");
        return Ok(());
    }
    let state = match status.state {
        deskfit::protocol::State::Unknown(b) => format!("state {b}"),
        s => format!("{s:?}").to_lowercase(),
    };
    let countdown = status.countdown.map(|c| format!("  countdown {c}")).unwrap_or_default();
    println!(
        "{state:<8} speed {}.{}  {:02}:{:02}  {}.{:02} km  {} steps  {} kcal{countdown}",
        status.speed / 10,
        status.speed % 10,
        status.elapsed_seconds / 60,
        status.elapsed_seconds % 60,
        status.distance_hundredths_km / 100,
        status.distance_hundredths_km % 100,
        status.steps,
        status.calories,
    );
    if out.raw {
        println!("         {}", treadmill.last_packet_hex());
    }
    Ok(())
}
