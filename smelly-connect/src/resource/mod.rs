mod model;

pub use crate::kernel::control::parse_resource_document as parse_resources;
pub use model::{DomainRule, IpRule, ResourceSet};
