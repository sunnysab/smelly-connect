use crate::resource::ResourceSet;

pub fn parse_resources(body: &str) -> Result<ResourceSet, roxmltree::Error> {
    crate::kernel::control::parse_resource_document(body)
}
