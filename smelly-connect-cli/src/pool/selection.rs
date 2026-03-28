use super::{AccountNode, PoolState};

pub(super) fn next_selectable_index(
    state: &mut PoolState,
    mut predicate: impl FnMut(&AccountNode) -> bool,
) -> Option<usize> {
    let total = state.nodes.len();
    if total == 0 {
        return None;
    }

    for offset in 0..total {
        let idx = (state.cursor + offset) % total;
        if predicate(&state.nodes[idx]) {
            state.cursor = idx + 1;
            return Some(idx);
        }
    }

    None
}
