//! `deskfit serve`: own one treadmill connection and talk JSON lines over
//! stdin/stdout with the Stream Deck plugin.
//!
//! Requests, one per line: `{"id": 1, "cmd": "toggle"}`; see
//! `controller::Request` for all commands. Every request gets a
//! `{"type": "reply", "id": 1, "ok": true}` line. Whenever anything changes
//! the service prints `{"type": "state", "connection": ..., "status": ...}`.
//! The service exits when stdin closes; it never moves the motor on its own.

use std::time::Duration;

use anyhow::{Result, bail};
use btleplug::api::{Central, CentralState};
use btleplug::platform::Adapter;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::ble::{self, Treadmill};
use crate::config;
use crate::controller::{Controller, Request, Snapshot};
use crate::protocol::Command;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Without a valid status for this long, the connection counts as lost.
const LIVENESS: Duration = Duration::from_secs(3);
const FIND_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Envelope {
    id: Option<u64>,
    #[serde(flatten)]
    request: Request,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Connection {
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    State {
        connection: Connection,
        error: Option<String>,
        #[serde(flatten)]
        snapshot: Snapshot,
    },
    Reply {
        id: Option<u64>,
        ok: bool,
        error: Option<String>,
    },
}

fn emit(event: &Event) {
    // Stdout is line buffered; each event is one line.
    println!("{}", serde_json::to_string(event).expect("events serialize"));
}

fn reply(id: Option<u64>, result: Result<(), String>) {
    emit(&Event::Reply {
        id,
        ok: result.is_ok(),
        error: result.err(),
    });
}

pub async fn serve() -> Result<()> {
    let (tx, mut requests) = mpsc::channel(64);
    tokio::spawn(read_requests(tx));
    let (adapter, _) = ble::adapter().await?;
    Service::default().run(&adapter, &mut requests).await
}

async fn read_requests(tx: mpsc::Sender<Envelope>) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Envelope>(&line) {
            Ok(envelope) => {
                if tx.send(envelope).await.is_err() {
                    break;
                }
            }
            Err(e) => reply(None, Err(format!("bad request: {e}"))),
        }
    }
    // Dropping tx tells the service to exit.
}

#[derive(Default)]
struct Service {
    controller: Controller,
    connection: Option<Connection>,
    error: Option<String>,
    last_state: String,
}

enum Ended {
    Exit,
    Lost(anyhow::Error),
}

impl Service {
    async fn run(&mut self, adapter: &Adapter, requests: &mut mpsc::Receiver<Envelope>) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        loop {
            self.set_connection(Connection::Connecting, None);
            let connecting = connect(adapter);
            tokio::pin!(connecting);
            let connected = loop {
                tokio::select! {
                    result = &mut connecting => break result,
                    request = requests.recv() => match request {
                        Some(r) => self.handle_disconnected(r),
                        None => return Ok(()),
                    },
                }
            };

            let error = match connected {
                Ok(mut treadmill) => {
                    backoff = Duration::from_secs(1);
                    self.set_connection(Connection::Connected, None);
                    let ended = self.session(&mut treadmill, requests).await;
                    self.controller.on_disconnect();
                    let _ = treadmill.disconnect().await;
                    match ended {
                        Ended::Exit => return Ok(()),
                        Ended::Lost(e) => e,
                    }
                }
                Err(e) => e,
            };
            eprintln!("treadmill connection: {error:#}");
            self.set_connection(Connection::Disconnected, Some(format!("{error:#}")));

            let retry_at = Instant::now() + backoff;
            backoff = (backoff * 2).min(MAX_BACKOFF);
            loop {
                tokio::select! {
                    _ = sleep_until(retry_at) => break,
                    request = requests.recv() => match request {
                        Some(r) => self.handle_disconnected(r),
                        None => return Ok(()),
                    },
                }
            }
        }
    }

    /// Settings apply at any time; anything that would move the motor is
    /// refused rather than queued for after reconnecting.
    fn handle_disconnected(&mut self, Envelope { id, request }: Envelope) {
        if let Request::Configure { .. } = request {
            let result = self.controller.handle(request, std::time::Instant::now()).map(|_| ());
            reply(id, result);
            self.emit_state();
        } else {
            reply(id, Err("not connected".into()));
        }
    }

    async fn session(&mut self, treadmill: &mut Treadmill, requests: &mut mpsc::Receiver<Envelope>) -> Ended {
        match self.session_inner(treadmill, requests).await {
            Ok(()) => Ended::Exit,
            Err(e) => Ended::Lost(e),
        }
    }

    /// Returns Ok when stdin closes and Err when the connection is lost.
    async fn session_inner(
        &mut self,
        treadmill: &mut Treadmill,
        requests: &mut mpsc::Receiver<Envelope>,
    ) -> Result<()> {
        let mut poll = tokio::time::interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_status = Instant::now();
        loop {
            let speed_write_at = self.controller.next_speed_write_at().map(Instant::from_std);
            tokio::select! {
                // Requests first, so a pause never waits behind polling.
                biased;
                request = requests.recv() => {
                    let Some(Envelope { id, request }) = request else { return Ok(()) };
                    match self.controller.handle(request, std::time::Instant::now()) {
                        Err(e) => reply(id, Err(e)),
                        Ok(None) => reply(id, Ok(())),
                        Ok(Some(command)) => {
                            if let Err(e) = treadmill.send(command).await {
                                reply(id, Err(format!("{e:#}")));
                                return Err(e);
                            }
                            reply(id, Ok(()));
                        }
                    }
                    self.write_due_speed(treadmill).await?;
                }
                _ = at(speed_write_at) => self.write_due_speed(treadmill).await?,
                status = treadmill.next_valid_status() => {
                    self.controller.on_status(status?, std::time::Instant::now());
                    last_status = Instant::now();
                }
                _ = poll.tick() => treadmill.send(Command::FetchStatus).await?,
                _ = sleep_until(last_status + LIVENESS) => bail!("no status for {LIVENESS:?}"),
            }
            self.emit_state();
        }
    }

    async fn write_due_speed(&mut self, treadmill: &Treadmill) -> Result<()> {
        if let Some(command) = self.controller.take_speed_write(std::time::Instant::now()) {
            treadmill.send(command).await?;
        }
        Ok(())
    }

    fn set_connection(&mut self, connection: Connection, error: Option<String>) {
        self.connection = Some(connection);
        self.error = error;
        self.emit_state();
    }

    /// Print the state if it differs from the last one printed.
    fn emit_state(&mut self) {
        let event = Event::State {
            connection: self.connection.unwrap_or(Connection::Disconnected),
            error: self.error.clone(),
            snapshot: self.controller.snapshot(std::time::Instant::now()),
        };
        let line = serde_json::to_string(&event).expect("events serialize");
        if line != self.last_state {
            println!("{line}");
            self.last_state = line;
        }
    }
}

async fn connect(adapter: &Adapter) -> Result<Treadmill> {
    let state = adapter.adapter_state().await?;
    if state != CentralState::PoweredOn {
        bail!("Bluetooth is {state:?}");
    }
    let config = config::load()?;
    let peripheral = ble::find(adapter, &config.peripheral_id, FIND_TIMEOUT).await?;
    Treadmill::connect(peripheral).await
}

async fn at(deadline: Option<Instant>) {
    match deadline {
        Some(d) => sleep_until(d).await,
        None => std::future::pending().await,
    }
}
