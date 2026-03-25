use std::time::Duration;

use super::connect_target::ConnectTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepalivePolicy {
    Disabled,
    Icmp {
        target: ConnectTarget,
        interval: Duration,
    },
}

impl KeepalivePolicy {
    pub fn icmp(target: impl Into<ConnectTarget>, interval: Duration) -> Self {
        Self::Icmp {
            target: target.into(),
            interval,
        }
    }
}
