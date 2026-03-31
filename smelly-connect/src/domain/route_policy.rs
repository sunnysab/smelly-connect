#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutePolicy {
    #[default]
    DirectNonResourceTargets,
    RejectNonResourceTargets,
}

impl RoutePolicy {
    pub fn direct_non_resource_targets() -> Self {
        Self::DirectNonResourceTargets
    }

    pub fn block_non_resource_targets() -> Self {
        Self::RejectNonResourceTargets
    }
}
