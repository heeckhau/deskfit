use std::time::Duration;

use anyhow::Result;
use btleplug::api::{Central, CentralState, Peripheral as _, ScanFilter};
use serde::Serialize;

use crate::ble;

#[derive(Debug, Serialize)]
pub struct ScanReport {
    pub adapter_state: String,
    pub devices: Vec<Device>,
}

#[derive(Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    /// Advertises the vendor service or a treadmill-like name.
    pub candidate: bool,
}

pub async fn scan(duration: Duration) -> Result<ScanReport> {
    let (adapter, state) = ble::adapter().await?;
    if state != CentralState::PoweredOn {
        return Ok(ScanReport {
            adapter_state: format!("{state:?}"),
            devices: Vec::new(),
        });
    }

    adapter.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(duration).await;
    adapter.stop_scan().await?;

    let mut devices = Vec::new();
    for peripheral in adapter.peripherals().await? {
        let Some(props) = peripheral.properties().await? else {
            continue;
        };
        devices.push(Device {
            id: peripheral.id().to_string(),
            name: ble::name(&props),
            rssi: props.rssi,
            candidate: ble::is_candidate(&props),
        });
    }
    devices.sort_by_key(|d| (!d.candidate, d.name.is_none(), -d.rssi.unwrap_or(i16::MIN / 2)));

    Ok(ScanReport {
        adapter_state: format!("{state:?}"),
        devices,
    })
}
