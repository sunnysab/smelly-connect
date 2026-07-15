use crate::resource::ResourceSet;

#[derive(Clone)]
pub struct ControlPlaneState {
    pub authorized_twfid: String,
    pub legacy_cipher_hint: Option<String>,
    pub resources: ResourceSet,
}
