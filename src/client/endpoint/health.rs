use std::time::{Duration, Instant};

pub(super) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
pub(super) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);
// Match the remote setup allowance without extending established transport health checks.
const INITIAL_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HealthAction {
    None,
    Ping,
    Expired,
}

pub(super) struct EndpointHealth {
    connected_at: Instant,
    last_received: Option<Instant>,
    ping_sent_at: Option<Instant>,
    ready: bool,
    probe_due: bool,
}

impl EndpointHealth {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            connected_at: now,
            last_received: None,
            ping_sent_at: None,
            ready: false,
            probe_due: false,
        }
    }

    pub(super) fn received(&mut self, now: Instant) {
        self.last_received = Some(now);
        self.ping_sent_at = None;
    }

    pub(super) fn ready(&mut self) {
        self.ready = true;
    }

    pub(super) fn resume(&mut self) {
        self.ping_sent_at = None;
        self.probe_due = true;
    }

    pub(super) fn action(&self, now: Instant) -> HealthAction {
        let initial_snapshot_expired = !self.ready
            && now.saturating_duration_since(self.connected_at) >= INITIAL_SNAPSHOT_TIMEOUT;
        // Welcome precedes state-loop registration, which can postpone the first pong.
        let probe_expired = (self.ready || self.last_received.is_some())
            && self
                .ping_sent_at
                .is_some_and(|sent_at| now.saturating_duration_since(sent_at) >= HEARTBEAT_TIMEOUT);
        if initial_snapshot_expired || probe_expired {
            HealthAction::Expired
        } else if self.probe_due
            || (self.ping_sent_at.is_none()
                && now.saturating_duration_since(self.last_received.unwrap_or(self.connected_at))
                    >= HEARTBEAT_INTERVAL)
        {
            HealthAction::Ping
        } else {
            HealthAction::None
        }
    }

    pub(super) fn ping_sent(&mut self, now: Instant) {
        self.probe_due = false;
        self.ping_sent_at = Some(now);
    }
}

/// andreconde fork (sheprd): smoothed round-trip time per endpoint, for the
/// sidebar. herdr only pings after HEARTBEAT_INTERVAL of silence, so a busy
/// link would never be measured; sheprd sends its own light probe every
/// RTT_PROBE_INTERVAL and times the matching pong.
pub(super) const RTT_PROBE_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Default)]
struct RttState {
    smoothed_ms: Option<f64>,
    probe_sent_at: Option<Instant>,
    last_probe: Option<Instant>,
}

fn rtt_store(
) -> &'static std::sync::Mutex<std::collections::HashMap<super::ClientEndpointId, RttState>> {
    static STORE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<super::ClientEndpointId, RttState>>,
    > = std::sync::OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// True when a latency probe should go out now; records it as sent.
pub(super) fn rtt_probe_due(endpoint_id: &super::ClientEndpointId, now: Instant) -> bool {
    // Process-wide state: keep it out of herdr's own health tests, which run in
    // parallel and count pings exactly.
    if cfg!(test) {
        return false;
    }
    let mut store = rtt_store().lock().unwrap_or_else(|e| e.into_inner());
    let state = store.entry(endpoint_id.clone()).or_default();
    let outstanding = state
        .probe_sent_at
        .is_some_and(|sent| now.saturating_duration_since(sent) < HEARTBEAT_TIMEOUT);
    let due = state
        .last_probe
        .is_none_or(|last| now.saturating_duration_since(last) >= RTT_PROBE_INTERVAL);
    if outstanding || !due {
        return false;
    }
    state.probe_sent_at = Some(now);
    state.last_probe = Some(now);
    true
}

pub(crate) fn rtt_pong(endpoint_id: &super::ClientEndpointId) {
    let mut store = rtt_store().lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = store.get_mut(endpoint_id) else {
        return;
    };
    let Some(sent) = state.probe_sent_at.take() else {
        return;
    };
    let sample = sent.elapsed().as_secs_f64() * 1000.0;
    state.smoothed_ms = Some(
        state
            .smoothed_ms
            .map_or(sample, |previous| previous * 0.7 + sample * 0.3),
    );
}

pub(crate) fn endpoint_rtt_ms(endpoint_id: &super::ClientEndpointId) -> Option<u32> {
    rtt_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(endpoint_id)
        .and_then(|state| state.smoothed_ms)
        .map(|ms| ms.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_connection_is_probed_then_expires_without_a_reply() {
        let now = Instant::now();
        let mut health = EndpointHealth::new(now);
        health.ready();
        assert_eq!(health.action(now), HealthAction::None);
        assert_eq!(health.action(now + HEARTBEAT_INTERVAL), HealthAction::Ping);
        health.ping_sent(now + HEARTBEAT_INTERVAL);
        assert_eq!(
            health.action(now + HEARTBEAT_INTERVAL + HEARTBEAT_TIMEOUT),
            HealthAction::Expired
        );
    }

    #[test]
    fn pause_reprobes_without_extending_initial_readiness_budget() {
        let now = Instant::now();
        let mut health = EndpointHealth::new(now);
        health.received(now);
        health.ping_sent(now);
        health.resume();
        assert_eq!(
            health.action(now + Duration::from_secs(59)),
            HealthAction::Ping
        );
        health.ping_sent(now + Duration::from_secs(59));
        health.resume();
        assert_eq!(
            health.action(now + Duration::from_secs(60)),
            HealthAction::Expired
        );
    }

    #[test]
    fn any_incoming_message_satisfies_an_outstanding_probe() {
        let now = Instant::now();
        let mut health = EndpointHealth::new(now);
        health.ready();
        health.ping_sent(now);
        health.received(now + HEARTBEAT_TIMEOUT - Duration::from_millis(1));
        assert_eq!(health.action(now + HEARTBEAT_TIMEOUT), HealthAction::None);
    }

    #[test]
    fn delayed_registration_can_reply_after_the_normal_probe_deadline() {
        let now = Instant::now();
        let mut health = EndpointHealth::new(now);
        health.ping_sent(now + HEARTBEAT_INTERVAL);
        let registered = now + Duration::from_secs(45);
        assert_eq!(health.action(registered), HealthAction::None);
        health.received(registered);
        health.ready();
        assert_eq!(health.action(registered), HealthAction::None);
    }

    #[test]
    fn responding_startup_gets_time_for_its_snapshot_but_cannot_wait_forever() {
        let now = Instant::now();
        let mut health = EndpointHealth::new(now);
        for seconds in (5..60).step_by(5) {
            let reply = now + Duration::from_secs(seconds);
            health.received(reply);
            assert_eq!(health.action(reply), HealthAction::None);
        }
        assert_eq!(
            health.action(now + Duration::from_secs(60)),
            HealthAction::Expired
        );
    }

    #[test]
    fn startup_silence_is_bounded_and_confirmed_transport_keeps_normal_probe_timeout() {
        let now = Instant::now();
        let mut health = EndpointHealth::new(now);
        health.ping_sent(now + HEARTBEAT_INTERVAL);
        assert_eq!(
            health.action(now + Duration::from_secs(60)),
            HealthAction::Expired
        );

        let mut health = EndpointHealth::new(now);
        health.received(now + Duration::from_secs(6));
        health.ping_sent(now + Duration::from_secs(11));
        assert_eq!(
            health.action(now + Duration::from_secs(20)),
            HealthAction::None
        );
        assert_eq!(
            health.action(now + Duration::from_secs(21)),
            HealthAction::Expired
        );
    }
}
