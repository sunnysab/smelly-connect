use std::time::Duration;

use super::snapshot::{PoolHealthStatus, PoolSummary};
use super::{AccountState, PoolState};

pub(super) fn next_backoff(backoff: Duration, max: Duration) -> Duration {
    let doubled = backoff.saturating_mul(2);
    if doubled > max {
        max
    } else {
        doubled
    }
}

pub(super) fn state_label(state: &AccountState) -> &'static str {
    match state {
        AccountState::Idle => "Idle",
        AccountState::Connecting => "Connecting",
        AccountState::Active(_) => "Active",
        AccountState::Dead => "Dead",
        AccountState::Disabled => "Disabled",
    }
}

pub(super) fn build_pool_summary(state: &PoolState) -> PoolSummary {
    let mut active_nodes = 0;
    let mut connecting_nodes = 0;
    let mut idle_nodes = 0;
    let mut dead_nodes = 0;
    let mut disabled_nodes = 0;

    for node in &state.nodes {
        match node.state {
            AccountState::Active(_) => active_nodes += 1,
            AccountState::Connecting => connecting_nodes += 1,
            AccountState::Idle => idle_nodes += 1,
            AccountState::Dead => dead_nodes += 1,
            AccountState::Disabled => disabled_nodes += 1,
        }
    }

    let selectable_nodes = active_nodes;
    let status = if selectable_nodes > 0 {
        PoolHealthStatus::Healthy
    } else if connecting_nodes > 0 || idle_nodes > 0 || dead_nodes > 0 {
        PoolHealthStatus::Recovering
    } else {
        PoolHealthStatus::Down
    };

    PoolSummary {
        status,
        total_nodes: state.nodes.len(),
        selectable_nodes,
        active_nodes,
        connecting_nodes,
        idle_nodes,
        dead_nodes,
        disabled_nodes,
        total_reconnections: state.total_reconnections,
    }
}


