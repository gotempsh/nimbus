use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub id: String,
    pub name: String,
    pub country: Option<String>,
}

/// A provider's instance size/plan, always priced so callers can show cost
/// before provisioning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceType {
    pub id: String,
    pub name: String,
    pub vcpus: u32,
    pub memory_gb: f32,
    pub disk_gb: u32,
    /// Gross monthly price in `currency`. Providers billing hourly are
    /// normalized to a 730h month so prices are comparable across backends.
    pub monthly_price: f64,
    /// ISO 4217 code of `monthly_price` — providers bill in different
    /// currencies (Hetzner EUR, Vultr USD) and pretending otherwise would
    /// misprice by the FX spread.
    pub currency: String,
}

/// A bootable OS image, discovered per region.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    /// Provider-specific identifier to pass as `CreateInstance.image`.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInstance {
    pub name: String,
    pub region: String,
    pub instance_type: String,
    /// Provider-specific OS image identifier (e.g. "ubuntu-24.04").
    pub image: String,
    pub ssh_public_key: String,
    /// Existing network to attach at boot, if the provider supports it.
    pub network_id: Option<String>,
    /// cloud-init user-data, if the provider supports it.
    pub user_data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub region: String,
    pub instance_type: String,
    pub status: InstanceStatus,
    /// May be empty until the provider assigns one.
    pub public_ipv4: Option<String>,
    pub private_ipv4: Option<String>,
    /// Default login user for the image family the adapter provisions
    /// (root on Hetzner/Vultr; ubuntu on OVH Ubuntu images).
    pub ssh_user: String,
    pub ssh_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    Provisioning,
    Running,
    Stopped,
    Deleting,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVolume {
    pub name: String,
    pub region: String,
    pub size_gb: u32,
    /// Attach immediately to this instance, if given.
    pub instance_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    pub id: String,
    pub name: String,
    pub region: String,
    pub size_gb: u32,
    pub attached_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateNetwork {
    pub name: String,
    pub region: String,
    /// CIDR range, e.g. "10.0.0.0/16".
    pub ip_range: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub region: String,
    pub ip_range: String,
}

/// A three-valued answer to "can this credential do X?".
///
/// `Unknown` is an answer, not a failure: several providers have no way to
/// read a credential's permissions, and the only other way to find out would
/// be to create something. Callers should treat it as "not ruled out".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Yes,
    No,
    Unknown,
}

/// What [`CloudProvider::check_credential`](crate::CloudProvider::check_credential)
/// could establish about a credential using read-only calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialCheck {
    /// The provider accepted the credential on an endpoint that requires one.
    pub authenticated: bool,
    /// Whether the credential can create instances, as far as the provider's
    /// API lets that be determined without creating anything.
    pub can_create_instances: Capability,
    /// The read-only requests the answer is based on, e.g. `"GET /account"`.
    pub checks: Vec<String>,
    /// What was found and why, including what could not be determined.
    /// Never contains the credential.
    pub detail: String,
}

impl CredentialCheck {
    pub(crate) fn rejected(checks: &[&str], detail: impl Into<String>) -> Self {
        Self {
            authenticated: false,
            can_create_instances: Capability::No,
            checks: checks.iter().map(|c| (*c).to_owned()).collect(),
            detail: detail.into(),
        }
    }

    pub(crate) fn accepted(
        can_create_instances: Capability,
        checks: &[&str],
        detail: impl Into<String>,
    ) -> Self {
        Self {
            authenticated: true,
            can_create_instances,
            checks: checks.iter().map(|c| (*c).to_owned()).collect(),
            detail: detail.into(),
        }
    }
}
