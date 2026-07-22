//! One async trait for instance, block-storage, and network provisioning
//! across cloud providers. Each backend lives in `providers::<name>` and
//! implements [`CloudProvider`]; nothing provider-specific leaks past it.

mod error;
pub mod providers;
mod types;

pub use error::{Error, Result};
pub use types::*;

use async_trait::async_trait;

#[async_trait]
pub trait CloudProvider: Send + Sync {
    /// Stable lowercase identifier, e.g. "hetzner", "vultr", "ovh".
    fn id(&self) -> &'static str;

    // -- discovery -----------------------------------------------------
    async fn regions(&self) -> Result<Vec<Region>>;
    async fn instance_types(&self, region: &str) -> Result<Vec<InstanceType>>;

    // -- instances -------------------------------------------------------
    async fn create_instance(&self, req: CreateInstance) -> Result<Instance>;
    async fn get_instance(&self, id: &str) -> Result<Instance>;
    async fn list_instances(&self) -> Result<Vec<Instance>>;
    async fn delete_instance(&self, id: &str) -> Result<()>;

    // -- storage ---------------------------------------------------------
    async fn create_volume(&self, req: CreateVolume) -> Result<Volume>;
    async fn list_volumes(&self) -> Result<Vec<Volume>>;
    async fn attach_volume(&self, volume_id: &str, instance_id: &str) -> Result<()>;
    async fn detach_volume(&self, volume_id: &str) -> Result<()>;
    async fn delete_volume(&self, id: &str) -> Result<()>;

    // -- networking --------------------------------------------------------
    async fn create_network(&self, req: CreateNetwork) -> Result<Network>;
    async fn list_networks(&self) -> Result<Vec<Network>>;
    async fn delete_network(&self, id: &str) -> Result<()>;
}
