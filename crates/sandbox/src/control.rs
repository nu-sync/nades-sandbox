use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use nades_control::{AppCommand, AppMessage, ControlError, ControlRequest};
use winit::event_loop::EventLoopProxy;

#[derive(Debug)]
pub enum UserEvent {
    Command(ControlRequest),
    Disconnected,
    /// A thread of the sandbox's has something for the window (lineups, a stage, a map, the game's effects).
    Wake,
}

pub struct Control {
    connected: Arc<AtomicBool>,
    disposition: Arc<Readiness>,
    pending: Arc<Mutex<PendingRequests>>,
}

#[derive(Default)]
struct Readiness(AtomicU8);

impl Readiness {
    const STARTING: u8 = 0;
    const READY: u8 = 1;
    const CANCELLED: u8 = 2;

    fn mark_ready(&self) -> bool {
        self.0
            .compare_exchange(
                Self::STARTING,
                Self::READY,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn detach(&self) -> bool {
        self.0
            .compare_exchange(
                Self::STARTING,
                Self::CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err_and(|state| state == Self::READY)
    }
}

#[derive(Default)]
struct PendingRequests(HashMap<u64, bool>);

impl PendingRequests {
    fn register(&mut self, id: u64) -> bool {
        if self.0.len() >= 128 || self.0.contains_key(&id) {
            return false;
        }
        self.0.insert(id, false);
        true
    }

    fn cancel(&mut self, id: u64) {
        if let Some(cancelled) = self.0.get_mut(&id) {
            *cancelled = true;
        }
    }

    fn begin(&mut self, request: &ControlRequest, unix_ms: u64) -> Result<(), ControlError> {
        if self.0.remove(&request.id).unwrap_or(true) {
            return Err(ControlError::new(
                "cancelled",
                "Request cancelled before application.",
                false,
            ));
        }
        if request
            .deadline_unix_ms
            .is_some_and(|deadline| deadline <= unix_ms)
        {
            return Err(ControlError::new(
                "timeout",
                "Request deadline expired before application; no change was applied.",
                true,
            ));
        }
        Ok(())
    }
}

impl Control {
    pub fn start(proxy: EventLoopProxy<UserEvent>) -> std::io::Result<Self> {
        let connected = Arc::new(AtomicBool::new(true));
        let disposition = Arc::new(Readiness::default());
        let lifecycle = disposition.clone();
        let pending = Arc::new(Mutex::new(PendingRequests::default()));
        let queued = pending.clone();
        let active = connected.clone();
        std::thread::Builder::new()
            .name("app-control".into())
            .spawn(move || {
                let stdin = std::io::stdin();
                for line in stdin.lock().lines() {
                    let Ok(line) = line else { break };
                    if line.len() > 64 * 1024 {
                        break;
                    }
                    let Ok(request) = serde_json::from_str::<ControlRequest>(&line) else {
                        break;
                    };
                    if let AppCommand::Cancel { request_id } = request.command {
                        queued.lock().expect("control pending").cancel(request_id);
                        continue;
                    }
                    if !queued.lock().expect("control pending").register(request.id) {
                        break;
                    }
                    if proxy.send_event(UserEvent::Command(request)).is_err() {
                        break;
                    }
                }
                active.store(false, Ordering::Release);
                let ready = lifecycle.detach();
                let _ = write(&AppMessage::Detached { ready });
                let _ = proxy.send_event(UserEvent::Disconnected);
            })?;
        Ok(Self {
            connected,
            disposition,
            pending,
        })
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn mark_ready(&self) -> bool {
        self.disposition.mark_ready()
    }

    pub fn begin(&self, request: &ControlRequest) -> Result<(), ControlError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| ControlError::new("clock_unavailable", error.to_string(), true))?
            .as_millis() as u64;
        self.pending
            .lock()
            .expect("control pending")
            .begin(request, now)
    }
}

pub fn write(message: &AppMessage) -> bool {
    let Ok(mut bytes) = serde_json::to_vec(message) else {
        return false;
    };
    bytes.push(b'\n');
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&bytes)
        .and_then(|()| stdout.flush())
        .is_ok()
}

pub fn check_revision(expected: Option<u64>, actual: u64) -> Result<(), ControlError> {
    if expected.is_some_and(|revision| revision != actual) {
        Err(ControlError::new(
            "stale_revision",
            format!(
                "Expected revision {expected:?}; current revision is {actual}. Read app_status and retry."
            ),
            true,
        ))
    } else {
        Ok(())
    }
}

pub fn capture_at_revision<T>(
    expected: Option<u64>,
    actual: u64,
    encode: impl FnOnce() -> Result<T, ControlError>,
) -> Result<T, ControlError> {
    check_revision(expected, actual)?;
    encode()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_and_readiness_have_one_winner() {
        let starting = Readiness::default();
        assert!(!starting.detach());
        assert!(!starting.mark_ready());
        let ready = Readiness::default();
        assert!(ready.mark_ready());
        assert!(ready.detach());
        assert!(ready.detach());
    }

    #[test]
    fn a_scene_change_during_draw_does_not_return_an_image_for_the_old_revision() {
        check_revision(Some(7), 7).unwrap();
        let mut encoded = false;
        let result = capture_at_revision(Some(7), 8, || {
            encoded = true;
            Ok(vec![137_u8, 80, 78, 71])
        });
        assert_eq!(result.unwrap_err().code, "stale_revision");
        assert!(!encoded);
        assert_eq!(
            capture_at_revision(Some(8), 8, || Ok(vec![137_u8, 80, 78, 71])).unwrap(),
            vec![137, 80, 78, 71]
        );
    }

    #[test]
    fn expired_and_cancelled_requests_never_begin_application() {
        let mut pending = PendingRequests::default();
        assert!(pending.register(1));
        let request = ControlRequest {
            id: 1,
            deadline_unix_ms: Some(1000),
            command: AppCommand::GetState,
        };
        assert_eq!(pending.begin(&request, 1000).unwrap_err().code, "timeout");
        assert!(pending.register(2));
        pending.cancel(2);
        let request = ControlRequest {
            id: 2,
            deadline_unix_ms: None,
            command: AppCommand::GetState,
        };
        assert_eq!(pending.begin(&request, 1000).unwrap_err().code, "cancelled");
        assert!(pending.register(3));
        let request = ControlRequest {
            id: 3,
            deadline_unix_ms: Some(1001),
            command: AppCommand::GetState,
        };
        pending.begin(&request, 1000).unwrap();
        pending.cancel(3);
        assert!(pending.0.is_empty());
    }
}
