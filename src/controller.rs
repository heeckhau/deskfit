//! Decisions of the helper service, kept free of I/O so they can be tested.
//!
//! The service feeds in status packets and requests with the current time;
//! the controller says which commands to write and when.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::protocol::{Command, Speed, State, Status};

/// A status older than this is not trusted for decisions.
pub const FRESH: Duration = Duration::from_millis(1500);
/// Minimum time between speed writes; rotation in between is coalesced.
pub const SPEED_WRITE_INTERVAL: Duration = Duration::from_millis(250);
/// How long a written target is used as the base for further adjustments,
/// covering the time until a status reflects it.
const COMMANDED_HOLD: Duration = Duration::from_millis(1500);
/// Stopped this long without a request, the service stops polling. Any
/// status request wakes the treadmill, which otherwise sleeps 10-15 s after
/// the last one.
pub const IDLE_AFTER: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Pause when running or starting; start when paused or stopped.
    Toggle,
    Start,
    Pause,
    Stop,
    /// Change the target speed by `delta` whole positions (1 km/h each), like
    /// the remote. The belt does not noticeably honor tenths in between.
    Adjust { delta: i16 },
    SetSpeed { speed: u8 },
    Configure { max_speed: u8 },
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    /// None when no fresh status is available: the belt state is unknown.
    pub status: Option<Status>,
    /// Speed requested but not yet confirmed by a status.
    pub target: Option<u8>,
    pub max_speed: u8,
    /// Stopped and no longer polled, so that the treadmill can sleep.
    pub idle: bool,
}

pub struct Controller {
    status: Option<(Status, Instant)>,
    pending: Option<Speed>,
    commanded: Option<(Speed, Instant)>,
    last_speed_write: Option<Instant>,
    max: Speed,
    stopped_since: Option<Instant>,
    last_request: Option<Instant>,
}

impl Default for Controller {
    fn default() -> Self {
        Controller {
            status: None,
            pending: None,
            commanded: None,
            last_speed_write: None,
            max: Speed::MAX,
            stopped_since: None,
            last_request: None,
        }
    }
}

impl Controller {
    pub fn fresh_status(&self, now: Instant) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|(_, at)| now.duration_since(*at) < FRESH)
            .map(|(s, _)| s)
    }

    pub fn on_status(&mut self, status: Status, now: Instant) {
        // Paused or stopped from elsewhere (e.g. the remote): drop queued speeds.
        if status.state != State::Running {
            self.pending = None;
            self.commanded = None;
        }
        if self.commanded.is_some_and(|(s, _)| s.raw() == status.speed) {
            self.commanded = None;
        }
        if status.state == State::Stopped {
            self.stopped_since.get_or_insert(now);
        } else {
            self.stopped_since = None;
        }
        self.status = Some((status, now));
    }

    /// Whether to stop polling: the belt has been stopped for a while and
    /// nobody asked for anything since.
    pub fn idle(&self, now: Instant) -> bool {
        let long_ago = |t: Instant| now.duration_since(t) >= IDLE_AFTER;
        self.stopped_since.is_some_and(long_ago) && self.last_request.is_none_or(long_ago)
    }

    /// Forget everything learned over the lost connection. Queued speed
    /// changes are stale and must not be sent after reconnecting.
    pub fn on_disconnect(&mut self) {
        self.status = None;
        self.pending = None;
        self.commanded = None;
        self.last_speed_write = None;
        self.stopped_since = None;
        self.last_request = None;
    }

    /// Handle a request. Returns a command to write immediately, if any.
    pub fn handle(&mut self, request: Request, now: Instant) -> Result<Option<Command>, String> {
        if !matches!(request, Request::Configure { .. }) {
            self.last_request = Some(now);
        }
        match request {
            Request::Toggle => {
                let state = self
                    .fresh_status(now)
                    .ok_or("treadmill state unknown")?
                    .state;
                match state {
                    State::Running | State::Starting => self.handle(Request::Pause, now),
                    State::Paused | State::Stopped => self.handle(Request::Start, now),
                    State::Unknown(b) => Err(format!("treadmill reports unknown state {b}")),
                }
            }
            Request::Start => Ok(Some(Command::Start)),
            Request::Pause => {
                self.pending = None;
                Ok(Some(Command::Pause))
            }
            Request::Stop => {
                self.pending = None;
                Ok(Some(Command::Stop))
            }
            Request::Adjust { delta } => {
                let base = i16::from(self.speed_base(now)?.raw());
                // From between positions, the first step lands on the next
                // whole position in the direction of travel.
                let position = if delta >= 0 { base / 10 } else { (base + 9) / 10 };
                self.queue_speed(position.saturating_add(delta).saturating_mul(10));
                Ok(None)
            }
            Request::SetSpeed { speed } => {
                self.speed_base(now)?;
                self.queue_speed(i16::from(speed));
                Ok(None)
            }
            Request::Configure { max_speed } => {
                self.max = Speed::new(max_speed.clamp(Speed::MIN.raw(), Speed::MAX.raw())).unwrap();
                Ok(None)
            }
        }
    }

    /// The speed an adjustment starts from. Speed changes are only accepted
    /// while running: whether set speed is safe while paused is untested.
    fn speed_base(&self, now: Instant) -> Result<Speed, String> {
        let status = self.fresh_status(now).ok_or("treadmill state unknown")?;
        if status.state != State::Running {
            return Err("speed can only be changed while running".into());
        }
        let recent = self
            .commanded
            .filter(|(_, at)| now.duration_since(*at) < COMMANDED_HOLD)
            .map(|(s, _)| s);
        Ok(self
            .pending
            .or(recent)
            .unwrap_or(Speed::new(status.speed).unwrap_or(Speed::MIN)))
    }

    fn queue_speed(&mut self, raw: i16) {
        let raw = raw.clamp(i16::from(Speed::MIN.raw()), i16::from(self.max.raw())) as u8;
        self.pending = Speed::new(raw);
    }

    /// When the next queued speed may be written, if one is queued.
    pub fn next_speed_write_at(&self) -> Option<Instant> {
        self.pending?;
        Some(self.last_speed_write.map_or_else(Instant::now, |t| t + SPEED_WRITE_INTERVAL))
    }

    /// Take the queued speed if it may be written now.
    pub fn take_speed_write(&mut self, now: Instant) -> Option<Command> {
        let speed = self.pending?;
        if self.last_speed_write.is_some_and(|t| now < t + SPEED_WRITE_INTERVAL) {
            return None;
        }
        self.pending = None;
        self.commanded = Some((speed, now));
        self.last_speed_write = Some(now);
        Some(Command::SetSpeed(speed))
    }

    pub fn snapshot(&self, now: Instant) -> Snapshot {
        let commanded = self
            .commanded
            .filter(|(_, at)| now.duration_since(*at) < COMMANDED_HOLD)
            .map(|(s, _)| s);
        Snapshot {
            status: self.fresh_status(now).cloned(),
            target: self.pending.or(commanded).map(Speed::raw),
            max_speed: self.max.raw(),
            idle: self.idle(now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: State, speed: u8) -> Status {
        Status {
            state,
            speed,
            elapsed_seconds: 0,
            distance_hundredths_km: 0,
            calories: 0,
            steps: 0,
            countdown: None,
        }
    }

    fn running_at(speed: u8, now: Instant) -> Controller {
        let mut c = Controller::default();
        c.on_status(status(State::Running, speed), now);
        c
    }

    fn speed(raw: u8) -> Command {
        Command::SetSpeed(Speed::new(raw).unwrap())
    }

    #[test]
    fn toggle_follows_fresh_state() {
        let t = Instant::now();
        let mut c = running_at(30, t);
        assert_eq!(c.handle(Request::Toggle, t), Ok(Some(Command::Pause)));
        c.on_status(status(State::Paused, 20), t);
        assert_eq!(c.handle(Request::Toggle, t), Ok(Some(Command::Start)));
        c.on_status(status(State::Starting, 0), t);
        assert_eq!(c.handle(Request::Toggle, t), Ok(Some(Command::Pause)));
        c.on_status(status(State::Stopped, 0), t);
        assert_eq!(c.handle(Request::Toggle, t), Ok(Some(Command::Start)));
    }

    #[test]
    fn toggle_refuses_unknown_or_stale_state() {
        let t = Instant::now();
        let mut c = Controller::default();
        assert!(c.handle(Request::Toggle, t).is_err());
        c.on_status(status(State::Running, 30), t);
        assert!(c.handle(Request::Toggle, t + FRESH).is_err());
        c.on_status(status(State::Unknown(9), 30), t);
        assert!(c.handle(Request::Toggle, t).is_err());
    }

    #[test]
    fn stale_status_is_reported_as_unknown() {
        let t = Instant::now();
        let c = running_at(30, t);
        assert!(c.snapshot(t).status.is_some());
        assert_eq!(c.snapshot(t + FRESH).status, None);
    }

    #[test]
    fn pause_is_allowed_without_known_state() {
        let mut c = Controller::default();
        assert_eq!(c.handle(Request::Pause, Instant::now()), Ok(Some(Command::Pause)));
    }

    #[test]
    fn rapid_rotation_is_coalesced() {
        let t = Instant::now();
        let mut c = running_at(30, t);
        for _ in 0..2 {
            c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        }
        assert_eq!(c.take_speed_write(t), Some(speed(50)));
        // More rotation within the write interval waits and coalesces.
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        let soon = t + SPEED_WRITE_INTERVAL / 2;
        assert_eq!(c.take_speed_write(soon), None);
        assert_eq!(c.next_speed_write_at(), Some(t + SPEED_WRITE_INTERVAL));
        assert_eq!(c.take_speed_write(t + SPEED_WRITE_INTERVAL), Some(speed(70)));
        assert_eq!(c.take_speed_write(t + SPEED_WRITE_INTERVAL * 2), None);
    }

    #[test]
    fn adjustments_build_on_last_commanded_target() {
        let t = Instant::now();
        let mut c = running_at(30, t);
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        c.take_speed_write(t);
        // The next status still shows the old speed.
        c.on_status(status(State::Running, 30), t);
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        assert_eq!(c.take_speed_write(t + SPEED_WRITE_INTERVAL), Some(speed(50)));
    }

    #[test]
    fn adjustment_from_between_positions_snaps_to_next_position() {
        let t = Instant::now();
        let mut c = running_at(35, t);
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        assert_eq!(c.take_speed_write(t), Some(speed(40)));
        let mut c = running_at(35, t);
        c.handle(Request::Adjust { delta: -1 }, t).unwrap();
        assert_eq!(c.take_speed_write(t), Some(speed(30)));
    }

    #[test]
    fn speed_is_bounded_by_configured_maximum() {
        let t = Instant::now();
        let mut c = running_at(30, t);
        c.handle(Request::Configure { max_speed: 50 }, t).unwrap();
        c.handle(Request::Adjust { delta: 100 }, t).unwrap();
        assert_eq!(c.take_speed_write(t), Some(speed(50)));
        c.on_status(status(State::Running, 50), t);
        c.handle(Request::Adjust { delta: -100 }, t + SPEED_WRITE_INTERVAL).unwrap();
        assert_eq!(c.take_speed_write(t + SPEED_WRITE_INTERVAL), Some(speed(10)));
        c.handle(Request::Configure { max_speed: 200 }, t).unwrap();
        assert_eq!(c.snapshot(t).max_speed, 80);
    }

    #[test]
    fn pause_discards_queued_speed() {
        let t = Instant::now();
        let mut c = running_at(30, t);
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        c.take_speed_write(t);
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        assert_eq!(c.handle(Request::Pause, t), Ok(Some(Command::Pause)));
        assert_eq!(c.take_speed_write(t + SPEED_WRITE_INTERVAL), None);
    }

    #[test]
    fn remote_pause_discards_queued_speed() {
        let t = Instant::now();
        let mut c = running_at(30, t);
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        c.on_status(status(State::Paused, 25), t);
        assert_eq!(c.take_speed_write(t), None);
    }

    #[test]
    fn rotation_while_not_running_is_refused() {
        let t = Instant::now();
        let mut c = Controller::default();
        c.on_status(status(State::Paused, 0), t);
        assert!(c.handle(Request::Adjust { delta: 1 }, t).is_err());
        assert!(c.handle(Request::SetSpeed { speed: 40 }, t).is_err());
        assert_eq!(c.take_speed_write(t), None);
    }

    #[test]
    fn idle_once_stopped_for_a_while() {
        let t = Instant::now();
        let mut c = Controller::default();
        c.on_status(status(State::Stopped, 0), t);
        // Later statuses do not restart the clock.
        c.on_status(status(State::Stopped, 0), t + IDLE_AFTER / 2);
        assert!(!c.idle(t + IDLE_AFTER / 2));
        assert!(c.idle(t + IDLE_AFTER));
        assert!(c.snapshot(t + IDLE_AFTER).idle);
    }

    #[test]
    fn requests_and_movement_end_idle() {
        let t = Instant::now();
        let mut c = Controller::default();
        c.on_status(status(State::Stopped, 0), t);
        let later = t + IDLE_AFTER;
        c.handle(Request::Configure { max_speed: 50 }, later).unwrap();
        assert!(c.idle(later));
        c.handle(Request::Start, later).unwrap();
        assert!(!c.idle(later));
        assert!(c.idle(later + IDLE_AFTER));
        c.on_status(status(State::Starting, 0), later + IDLE_AFTER);
        assert!(!c.idle(later + IDLE_AFTER * 2));
    }

    #[test]
    fn never_idle_while_paused_or_running() {
        let t = Instant::now();
        let mut c = running_at(30, t);
        assert!(!c.idle(t + IDLE_AFTER));
        c.on_status(status(State::Paused, 0), t);
        assert!(!c.idle(t + IDLE_AFTER));
    }

    #[test]
    fn disconnect_clears_queued_speed_and_state() {
        let t = Instant::now();
        let mut c = running_at(30, t);
        c.handle(Request::Adjust { delta: 1 }, t).unwrap();
        c.on_disconnect();
        assert_eq!(c.take_speed_write(t), None);
        assert_eq!(c.snapshot(t).status, None);
        assert!(c.handle(Request::Toggle, t).is_err());
        c.on_status(status(State::Stopped, 0), t);
        c.on_disconnect();
        assert!(!c.idle(t + IDLE_AFTER));
    }
}
