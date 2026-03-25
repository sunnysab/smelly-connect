#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutePolicy {
    #[default]
    RejectNonResourceTargets,
}
