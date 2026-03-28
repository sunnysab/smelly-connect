use tokio::time::Instant;

use super::{AccountFailure, AccountNode, AccountState, PoolHealthStatus, PoolState, PoolSummary};

pub(super) fn next_backoff(
    current: std::time::Duration,
    base: std::time::Duration,
    max: std::time::Duration,
) -> std::time::Duration {
    let doubled = current.saturating_mul(2);
    if doubled < base {
        base
    } else if doubled > max {
        max
    } else {
        doubled
    }
}

pub(super) fn state_label(state: &AccountState) -> &'static str {
    match state {
        AccountState::Configured(_) => "Configured",
        AccountState::Connecting => "Connecting",
        AccountState::Ready(_) => "Ready",
        AccountState::Suspect(_) => "Suspect",
        AccountState::Open(_) => "Open",
        AccountState::HalfOpen(_) => "HalfOpen",
    }
}

pub(super) fn build_pool_summary(state: &PoolState) -> PoolSummary {
    let mut ready_nodes = 0;
    let mut suspect_nodes = 0;
    let mut open_nodes = 0;
    let mut timed_open_nodes = 0;
    let mut half_open_nodes = 0;
    let mut connecting_nodes = 0;
    let mut configured_nodes = 0;

    for node in &state.nodes {
        match node.state {
            AccountState::Configured(_) => configured_nodes += 1,
            AccountState::Connecting => connecting_nodes += 1,
            AccountState::Ready(_) => ready_nodes += 1,
            AccountState::Suspect(_) => suspect_nodes += 1,
            AccountState::Open(_) => {
                open_nodes += 1;
                if node.open_until.is_some() {
                    timed_open_nodes += 1;
                }
            }
            AccountState::HalfOpen(_) => half_open_nodes += 1,
        }
    }

    let selectable_nodes = ready_nodes + suspect_nodes;
    let status = if selectable_nodes > 0 {
        PoolHealthStatus::Healthy
    } else if half_open_nodes > 0
        || connecting_nodes > 0
        || timed_open_nodes > 0
        || configured_nodes > 0
    {
        PoolHealthStatus::Recovering
    } else {
        PoolHealthStatus::Down
    };

    PoolSummary {
        status,
        total_nodes: state.nodes.len(),
        selectable_nodes,
        ready_nodes,
        suspect_nodes,
        open_nodes,
        half_open_nodes,
        connecting_nodes,
        configured_nodes,
    }
}

pub(super) fn open_node(node: &mut AccountNode, message: String) {
    node.current_backoff = next_backoff(node.current_backoff, node.backoff_base, node.backoff_max);
    node.open_until = Some(Instant::now() + node.current_backoff);
    node.live_probe_in_flight = false;
    node.state = AccountState::Open(AccountFailure { message });
}
