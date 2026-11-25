//! One async trait for instance, block-storage, and network provisioning
//! across cloud providers. Each backend lives in `providers::<name>` and
//! implements [`CloudProvider`]; nothing provider-specific leaks past it.

mod catalog;
mod error;
pub mod providers;
mod types;

pub use catalog::{provider_info, Caveats, CredentialField, ProviderInfo, PROVIDERS};
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
    async fn images(&self, region: &str) -> Result<Vec<Image>>;

    /// Static facts about this provider — the credential fields it needs and
    /// the caveats a caller must know before offering it (no delete API,
    /// no prices, irreversible create, ...). `None` for an adapter that is
    /// not in [`PROVIDERS`].
    fn info(&self) -> Option<&'static ProviderInfo> {
        provider_info(self.id())
    }

    /// Cheap authenticated call — lets callers fail fast on a bad or
    /// mis-scoped credential before storing it or enqueueing work.
    ///
    /// `Ok(())` means the provider accepted the credential; a rejected one is
    /// [`Error::Auth`]. Use [`check_credential`](Self::check_credential) to
    /// also learn whether it can create instances.
    async fn verify(&self) -> Result<()> {
        if self.check_credential().await?.authenticated {
            Ok(())
        } else {
            Err(Error::Auth {
                provider: self.id(),
            })
        }
    }

    /// Reports whether the credential authenticates and whether it can
    /// create instances, using only read-only requests: nothing is created,
    /// modified or deleted, so it is safe to run against a billed account.
    ///
    /// A credential the provider rejects is `Ok` with `authenticated: false`;
    /// `Err` means the check itself could not be completed (network failure,
    /// unexpected response).
    ///
    /// Every built-in adapter overrides this with the strongest check its
    /// provider's API allows (an account or token-introspection endpoint
    /// where one exists). This default only proves that `regions()` succeeds,
    /// which on some providers needs no credential at all — an adapter should
    /// not rely on it.
    async fn check_credential(&self) -> Result<CredentialCheck> {
        let checks = ["regions()"];
        match self.regions().await {
            Ok(_) => Ok(CredentialCheck::accepted(
                Capability::Unknown,
                &checks,
                "Listing regions succeeded. This adapter implements no stronger check, so this does not prove the credential is valid for anything else, nor that it can create instances.",
            )),
            Err(Error::Auth { .. }) => Ok(CredentialCheck::rejected(
                &checks,
                "The provider rejected the credential when listing regions.",
            )),
            Err(e) => Err(e),
        }
    }

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
