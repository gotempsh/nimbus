//! Hostinger VPS adapter. API docs: https://developers.hostinger.com/
//! (OpenAPI spec: https://github.com/hostinger/api). Flat REST + bearer
//! token, no request signing.
//!
//! Hostinger's VPS API is shaped very differently from the other six
//! adapters here, and callers need to know before reaching for it:
//!
//! - `create_instance` calls `POST /vps/v1/virtual-machines`, which is
//!   documented as "Purchase new virtual machine" — it creates a real
//!   billing subscription (optionally charging `payment_method_id`, or the
//!   account's default), not a metered pay-as-you-go instance.
//! - There is no delete/cancel endpoint for a VPS in the API at all,
//!   because a purchased VM is a subscription: deletion happens only by
//!   cancelling the subscription from the billing dashboard. `delete_instance`
//!   returns an explicit error rather than pretending to support it.
//! - There is no block-storage or private-networking product/API — disk is
//!   fixed per plan. `create_volume`/`create_network` (and the mutating
//!   volume/network calls) return an explicit error; the list calls return
//!   an empty list, which is simply true.
//! - The billing catalog (used for `instance_types`) does not expose
//!   vcpu/memory/disk specs ahead of purchase — those only appear on a VM
//!   resource after it exists. Hostinger's VPS hardware is fixed per plan
//!   tier and identical across plan families (KVM and Game Panel share
//!   hardware, differing only in pre-installed software), so `vcpus`/
//!   `memory_gb`/`disk_gb` are filled in from a static table in
//!   [`known_specs`], keyed by the tier number in the plan's display name
//!   (e.g. "KVM 2", "Game Panel 2" -> tier 2). A plan whose name doesn't
//!   match a known tier falls back to 0, same treatment as OVH's pending
//!   `monthly_price`.

use crate::{
    CloudProvider, CreateInstance, CreateNetwork, CreateVolume, Error, Image, Instance,
    InstanceStatus, InstanceType, Network, Region, Result, Volume,
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

const BASE: &str = "https://developers.hostinger.com/api";
const PROVIDER: &str = "hostinger";

pub struct Hostinger {
    token: String,
    base: String,
    client: Client,
}

impl Hostinger {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            base: BASE.to_owned(),
            client: Client::new(),
        }
    }

    /// Point at a different host — e.g. a local mock server for testing.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(Error::Auth { provider: PROVIDER });
        }
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(Error::Api {
                provider: PROVIDER,
                status: status.as_u16(),
                message: text,
            });
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| Error::Api {
            provider: PROVIDER,
            status: status.as_u16(),
            message: e.to_string(),
        })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, path, None).await
    }
}

/// (vcpus, memory_gb, disk_gb) for a known Hostinger VPS plan tier, parsed
/// from the plan's display name (e.g. "KVM 2", "Game Panel 2 (billed every
/// month)" both resolve to tier 2). Specs verified against
/// https://www.hostinger.com/vps-hosting and
/// https://www.hostinger.com/minecraft-server-hosting (2026-08-24) — both
/// plan families share identical hardware per tier.
fn known_specs(name: &str) -> Option<(u32, f32, u32)> {
    let tier: u32 = name.split_whitespace().find_map(|tok| tok.parse().ok())?;
    match tier {
        1 => Some((1, 4.0, 50)),
        2 => Some((2, 8.0, 100)),
        4 => Some((4, 16.0, 200)),
        8 => Some((8, 32.0, 400)),
        _ => None,
    }
}

fn instance_status(s: &str) -> InstanceStatus {
    match s {
        "running" => InstanceStatus::Running,
        "stopped" | "suspended" => InstanceStatus::Stopped,
        "starting" | "stopping" | "creating" | "initial" | "recreating" | "restoring"
        | "suspending" | "unsuspending" | "recovery" | "stopping_recovery" => {
            InstanceStatus::Provisioning
        }
        "destroying" | "destroyed" => InstanceStatus::Deleting,
        _ => InstanceStatus::Error,
    }
}

fn parse_instance(v: &Value) -> Instance {
    Instance {
        id: v["id"]
            .as_u64()
            .map(|id| id.to_string())
            .unwrap_or_default(),
        name: v["hostname"].as_str().unwrap_or_default().to_owned(),
        region: v["data_center_id"]
            .as_u64()
            .map(|d| d.to_string())
            .unwrap_or_default(),
        instance_type: v["plan"].as_str().unwrap_or_default().to_owned(),
        status: instance_status(v["state"].as_str().unwrap_or_default()),
        public_ipv4: v["ipv4"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|ip| ip["address"].as_str())
            .map(str::to_owned),
        // Hostinger VPS has no private networking product.
        private_ipv4: None,
        ssh_user: "root".to_owned(),
        ssh_port: 22,
    }
}

#[async_trait]
impl CloudProvider for Hostinger {
    fn id(&self) -> &'static str {
        PROVIDER
    }

    async fn regions(&self) -> Result<Vec<Region>> {
        let v = self.get("/vps/v1/data-centers").await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|r| Region {
                id: r["id"]
                    .as_u64()
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                name: r["city"]
                    .as_str()
                    .or(r["name"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                country: r["location"].as_str().map(str::to_uppercase),
            })
            .collect())
    }

    async fn instance_types(&self, _region: &str) -> Result<Vec<InstanceType>> {
        // Plans aren't region-scoped in the catalog — the same item_id is
        // purchasable in any data center.
        let v = self.get("/billing/v1/catalog?category=VPS").await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|item| {
                item["prices"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    // one InstanceType per plan — skip the daily/weekly/yearly
                    // duplicates of the same plan.
                    .filter(|p| p["period_unit"].as_str() == Some("month") && p["period"] == 1)
                    .map(|p| {
                        let (vcpus, memory_gb, disk_gb) =
                            known_specs(item["name"].as_str().unwrap_or_default())
                                .unwrap_or((0, 0.0, 0));
                        InstanceType {
                            id: p["id"].as_str().unwrap_or_default().to_owned(),
                            name: p["name"]
                                .as_str()
                                .or(item["name"].as_str())
                                .unwrap_or_default()
                                .to_owned(),
                            vcpus,
                            memory_gb,
                            disk_gb,
                            monthly_price: p["price"].as_f64().unwrap_or_default() / 100.0,
                            currency: p["currency"].as_str().unwrap_or("USD").to_owned(),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect())
    }

    async fn images(&self, _region: &str) -> Result<Vec<Image>> {
        let v = self.get("/vps/v1/templates").await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|t| Image {
                id: t["id"]
                    .as_u64()
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                name: t["name"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    async fn create_instance(&self, req: CreateInstance) -> Result<Instance> {
        if req.network_id.is_some() {
            return Err(Error::InvalidRequest(
                "hostinger: attaching a private network at create time is not supported (no private networking API)".into(),
            ));
        }
        if req.user_data.is_some() {
            return Err(Error::InvalidRequest(
                "hostinger: cloud-init user-data is not supported by the VPS setup API".into(),
            ));
        }
        let data_center_id: u64 = req.region.parse().map_err(|_| {
            Error::InvalidRequest("hostinger: region must be a numeric data center id".into())
        })?;
        let template_id: u64 = req.image.parse().map_err(|_| {
            Error::InvalidRequest("hostinger: image must be a numeric template id".into())
        })?;
        let body = json!({
            "item_id": req.instance_type,
            "setup": {
                "data_center_id": data_center_id,
                "template_id": template_id,
                "hostname": req.name,
                "public_key": {
                    "name": format!("{}-key", req.name),
                    "key": req.ssh_public_key.trim(),
                },
            },
        });
        let v = self
            .request(
                reqwest::Method::POST,
                "/vps/v1/virtual-machines",
                Some(body),
            )
            .await?;
        Ok(parse_instance(&v["virtual_machine"]))
    }

    async fn get_instance(&self, id: &str) -> Result<Instance> {
        let v = self.get(&format!("/vps/v1/virtual-machines/{id}")).await?;
        if v["id"].is_null() {
            return Err(Error::NotFound {
                provider: PROVIDER,
                resource: "instance",
                id: id.to_owned(),
            });
        }
        Ok(parse_instance(&v))
    }

    async fn list_instances(&self) -> Result<Vec<Instance>> {
        let v = self.get("/vps/v1/virtual-machines").await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(parse_instance)
            .collect())
    }

    async fn delete_instance(&self, _id: &str) -> Result<()> {
        Err(Error::InvalidRequest(
            "hostinger: deleting a VPS is not exposed via the API — cancel the underlying subscription from the billing dashboard instead".into(),
        ))
    }

    async fn create_volume(&self, _req: CreateVolume) -> Result<Volume> {
        Err(Error::InvalidRequest(
            "hostinger: block storage is not exposed via the API — VPS disk size is fixed per plan"
                .into(),
        ))
    }

    async fn list_volumes(&self) -> Result<Vec<Volume>> {
        Ok(Vec::new())
    }

    async fn attach_volume(&self, _volume_id: &str, _instance_id: &str) -> Result<()> {
        Err(Error::InvalidRequest(
            "hostinger: block storage is not exposed via the API".into(),
        ))
    }

    async fn detach_volume(&self, _volume_id: &str) -> Result<()> {
        Err(Error::InvalidRequest(
            "hostinger: block storage is not exposed via the API".into(),
        ))
    }

    async fn delete_volume(&self, _id: &str) -> Result<()> {
        Err(Error::InvalidRequest(
            "hostinger: block storage is not exposed via the API".into(),
        ))
    }

    async fn create_network(&self, _req: CreateNetwork) -> Result<Network> {
        Err(Error::InvalidRequest(
            "hostinger: private networking is not exposed via the API".into(),
        ))
    }

    async fn list_networks(&self) -> Result<Vec<Network>> {
        Ok(Vec::new())
    }

    async fn delete_network(&self, _id: &str) -> Result<()> {
        Err(Error::InvalidRequest(
            "hostinger: private networking is not exposed via the API".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::known_specs;

    #[test]
    fn known_specs_matches_kvm_tiers() {
        assert_eq!(known_specs("KVM 1"), Some((1, 4.0, 50)));
        assert_eq!(known_specs("KVM 2"), Some((2, 8.0, 100)));
        assert_eq!(known_specs("KVM 4"), Some((4, 16.0, 200)));
        assert_eq!(known_specs("KVM 8"), Some((8, 32.0, 400)));
    }

    #[test]
    fn known_specs_matches_game_panel_tiers_same_as_kvm() {
        assert_eq!(known_specs("Game Panel 1"), known_specs("KVM 1"));
        assert_eq!(known_specs("Game Panel 2"), known_specs("KVM 2"));
        assert_eq!(known_specs("Game Panel 4"), known_specs("KVM 4"));
        assert_eq!(known_specs("Game Panel 8"), known_specs("KVM 8"));
    }

    #[test]
    fn known_specs_ignores_billing_suffix() {
        assert_eq!(
            known_specs("KVM 2 (billed every month)"),
            Some((2, 8.0, 100))
        );
    }

    #[test]
    fn known_specs_unknown_plan_returns_none() {
        assert_eq!(known_specs("Shared Hosting Premium"), None);
        assert_eq!(known_specs(""), None);
    }
}
