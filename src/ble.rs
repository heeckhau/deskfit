use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use btleplug::api::{
    Central, CentralEvent, CentralState, Characteristic, Manager as _, Peripheral as _, PeripheralProperties,
    ScanFilter, ValueNotification, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::{Stream, StreamExt};
use uuid::Uuid;

use crate::protocol::{COMMAND_CHARACTERISTIC, Command, STATUS_CHARACTERISTIC, Status, decode_status};

pub const VENDOR_SERVICE: Uuid = Uuid::from_u128(0x0000fff0_0000_1000_8000_00805f9b34fb);
const NAME_PREFIX: &str = "V-RUN";

/// The first Bluetooth adapter and its state once CoreBluetooth has settled.
/// The state stays Unknown when permission is denied.
pub async fn adapter() -> Result<(Adapter, CentralState)> {
    let manager = Manager::new().await.context("creating Bluetooth manager")?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Bluetooth adapter"))?;
    let mut state = adapter.adapter_state().await?;
    for _ in 0..20 {
        if state != CentralState::Unknown {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        state = adapter.adapter_state().await?;
    }
    Ok((adapter, state))
}

pub fn name(props: &PeripheralProperties) -> Option<String> {
    props.local_name.clone().or_else(|| props.advertisement_name.clone())
}

/// Advertises the vendor service or a treadmill-like name.
pub fn is_candidate(props: &PeripheralProperties) -> bool {
    props.services.contains(&VENDOR_SERVICE) || name(props).is_some_and(|n| n.starts_with(NAME_PREFIX))
}

/// Scan until the peripheral with this identifier shows up.
pub async fn find(adapter: &Adapter, id: &str, timeout: Duration) -> Result<Peripheral> {
    let mut events = adapter.events().await?;
    adapter.start_scan(ScanFilter::default()).await?;
    let found = tokio::time::timeout(timeout, async {
        // Already known from an earlier scan in this process.
        for p in adapter.peripherals().await? {
            if p.id().to_string().eq_ignore_ascii_case(id) {
                return Ok(p);
            }
        }
        while let Some(event) = events.next().await {
            if let CentralEvent::DeviceDiscovered(pid) | CentralEvent::DeviceUpdated(pid) = event
                && pid.to_string().eq_ignore_ascii_case(id)
            {
                return adapter.peripheral(&pid).await.map_err(anyhow::Error::from);
            }
        }
        bail!("Bluetooth event stream ended")
    })
    .await;
    adapter.stop_scan().await?;
    found.map_err(|_| anyhow!("treadmill {id} not found within {timeout:?}; is it powered on?"))?
}

type Notifications = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

/// An open connection to the treadmill.
pub struct Treadmill {
    peripheral: Peripheral,
    command: Characteristic,
    notifications: Notifications,
    last_packet: Vec<u8>,
}

impl Treadmill {
    pub async fn connect(peripheral: Peripheral) -> Result<Treadmill> {
        peripheral
            .connect_with_timeout(Duration::from_secs(10))
            .await
            .context("connecting")?;
        peripheral
            .discover_services_with_timeout(Duration::from_secs(10))
            .await
            .context("discovering services")?;
        let characteristic = |uuid| {
            peripheral
                .characteristics()
                .into_iter()
                .find(|c| c.uuid == uuid)
                .ok_or_else(|| anyhow!("characteristic {uuid} not found"))
        };
        let command = characteristic(COMMAND_CHARACTERISTIC)?;
        let status = characteristic(STATUS_CHARACTERISTIC)?;
        let notifications = peripheral.notifications().await?;
        peripheral.subscribe(&status).await.context("subscribing to status")?;
        Ok(Treadmill {
            peripheral,
            command,
            notifications,
            last_packet: Vec::new(),
        })
    }

    /// The raw bytes of the last valid status packet, in hex.
    pub fn last_packet_hex(&self) -> String {
        hex(&self.last_packet)
    }

    /// Write a command. An acknowledged write does not mean the motor obeyed;
    /// check the reported status.
    pub async fn send(&self, command: Command) -> Result<()> {
        self.peripheral
            .write(&self.command, &command.encode(), WriteType::WithResponse)
            .await
            .with_context(|| format!("writing {command:?}"))
    }

    /// Wait for the next valid status notification. Invalid packets are
    /// reported on stderr and skipped.
    pub async fn next_status(&mut self, timeout: Duration) -> Result<Status> {
        tokio::time::timeout(timeout, self.next_valid_status())
            .await
            .map_err(|_| anyhow!("no valid status within {timeout:?}"))?
    }

    /// Like `next_status` without a timeout. Cancel-safe.
    pub async fn next_valid_status(&mut self) -> Result<Status> {
        while let Some(n) = self.notifications.next().await {
            if n.uuid != STATUS_CHARACTERISTIC {
                continue;
            }
            match decode_status(&n.value) {
                Ok(status) => {
                    self.last_packet = n.value;
                    return Ok(status);
                }
                Err(e) => eprintln!("ignoring status packet {}: {e}", hex(&n.value)),
            }
        }
        bail!("connection closed")
    }

    /// Request a status update and wait for it.
    pub async fn fetch_status(&mut self, timeout: Duration) -> Result<Status> {
        self.send(Command::FetchStatus).await?;
        self.next_status(timeout).await
    }

    pub async fn is_connected(&self) -> Result<bool> {
        self.peripheral.is_connected().await.map_err(Into::into)
    }

    pub async fn disconnect(self) -> Result<()> {
        self.peripheral.disconnect().await.map_err(Into::into)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}
