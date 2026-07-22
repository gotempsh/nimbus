//! Scaleway adapter. API docs: https://www.scaleway.com/en/developers/api/
//! REST + `X-Auth-Token` header. The Instance API is zone-scoped
//! (fr-par-1, nl-ams-1, ...), so a client is bound to one zone at
//! construction; `regions()` still lists all known zones for discovery.
//! Servers are created stopped and powered on explicitly; deletion uses the
//! `terminate` action (which also releases the server's volumes).

use crate::{
    CloudProvider, CreateInstance, CreateNetwork, CreateVolume, Error, Image, Instance,
    InstanceStatus, InstanceType, Network, Region, Result, Volume,
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

const BASE: &str = "https://api.scaleway.com";
const PROVIDER: &str = "scaleway";

/// All Instance API zones as of 2026. Scaleway has no unauthenticated
/// zone-discovery endpoint; this list changes rarely.
const ZONES: &[(&str, &str, &str)] = &[
    ("fr-par-1", "Paris 1", "FR"),
    ("fr-par-2", "Paris 2", "FR"),
    ("fr-par-3", "Paris 3", "FR"),
    ("nl-ams-1", "Amsterdam 1", "NL"),
    ("nl-ams-2", "Amsterdam 2", "NL"),
    ("nl-ams-3", "Amsterdam 3", "NL"),
    ("pl-waw-1", "Warsaw 1", "PL"),
    ("pl-waw-2", "Warsaw 2", "PL"),
    ("pl-waw-3", "Warsaw 3", "PL"),
];

pub struct Scaleway {
    secret_key: String,
    project_id: String,
    zone: String,
    base: String,
    client: Client,
}

impl Scaleway {
    pub fn new(
        secret_key: impl Into<String>,
        project_id: impl Into<String>,
        zone: impl Into<String>,
    ) -> Self {
        Self {
            secret_key: secret_key.into(),
            project_id: project_id.into(),
            zone: zone.into(),
            base: BASE.to_owned(),
            client: Client::new(),
        }
    }

    /// Point at a different host — e.g. a local mock server for testing.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// The VPC API is region-scoped (fr-par), not zone-scoped (fr-par-1).
    fn vpc_region(&self) -> String {
        self.zone
            .rsplit_once('-')
            .map(|(region, _)| region)
            .unwrap_or(&self.zone)
            .to_owned()
    }

    fn instance_path(&self, suffix: &str) -> String {
        format!("/instance/v1/zones/{}{suffix}", self.zone)
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
            .header("X-Auth-Token", &self.secret_key);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(Error::Auth { provider: PROVIDER });
        }
        if status == StatusCode::NO_CONTENT {
            return Ok(Value::Null);
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

    async fn server_action(&self, id: &str, action: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            &self.instance_path(&format!("/servers/{id}/action")),
            Some(json!({ "action": action })),
        )
        .await?;
        Ok(())
    }
}

fn instance_status(s: &str) -> InstanceStatus {
    match s {
        "running" => InstanceStatus::Running,
        "stopped" | "stopped in place" => InstanceStatus::Stopped,
        "starting" | "stopping" | "provisioning" => InstanceStatus::Provisioning,
        _ => InstanceStatus::Error,
    }
}

fn parse_instance(v: &Value) -> Instance {
    Instance {
        id: v["id"].as_str().unwrap_or_default().to_owned(),
        name: v["name"].as_str().unwrap_or_default().to_owned(),
        region: v["zone"].as_str().unwrap_or_default().to_owned(),
        instance_type: v["commercial_type"].as_str().unwrap_or_default().to_owned(),
        status: instance_status(v["state"].as_str().unwrap_or_default()),
        public_ipv4: v["public_ip"]["address"].as_str().map(str::to_owned),
        private_ipv4: v["private_ip"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        ssh_user: "root".to_owned(),
        ssh_port: 22,
    }
}

#[async_trait]
impl CloudProvider for Scaleway {
    fn id(&self) -> &'static str {
        PROVIDER
    }

    async fn regions(&self) -> Result<Vec<Region>> {
        Ok(ZONES
            .iter()
            .map(|(id, name, country)| Region {
                id: (*id).to_owned(),
                name: (*name).to_owned(),
                country: Some((*country).to_owned()),
            })
            .collect())
    }

    async fn instance_types(&self, _region: &str) -> Result<Vec<InstanceType>> {
        let v = self.get(&self.instance_path("/products/servers")).await?;
        let Some(servers) = v["servers"].as_object() else {
            return Ok(vec![]);
        };
        Ok(servers
            .iter()
            .map(|(id, t)| {
                let monthly = t["monthly_price"]
                    .as_f64()
                    .unwrap_or_else(|| t["hourly_price"].as_f64().unwrap_or(0.0) * 730.0);
                InstanceType {
                    id: id.clone(),
                    name: id.clone(),
                    vcpus: t["ncpus"].as_u64().unwrap_or_default() as u32,
                    memory_gb: t["ram"].as_f64().unwrap_or_default() as f32 / 1_073_741_824.0,
                    disk_gb: (t["volumes_constraint"]["min_size"]
                        .as_u64()
                        .unwrap_or_default()
                        / 1_000_000_000) as u32,
                    monthly_price: monthly,
                    currency: "EUR".to_owned(),
                }
            })
            .collect())
    }

    async fn images(&self, _region: &str) -> Result<Vec<Image>> {
        let v = self
            .get(&self.instance_path("/images?public=true&per_page=100"))
            .await?;
        Ok(v["images"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|i| Image {
                id: i["id"].as_str().unwrap_or_default().to_owned(),
                name: i["name"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    async fn create_instance(&self, req: CreateInstance) -> Result<Instance> {
        // SSH keys live at the IAM level and are injected into instances of
        // the project at boot. Tolerate duplicates.
        match self
            .request(
                reqwest::Method::POST,
                "/iam/v1alpha1/ssh-keys",
                Some(json!({
                    "name": format!("{}-key", req.name),
                    "public_key": req.ssh_public_key.trim(),
                    "project_id": self.project_id,
                })),
            )
            .await
        {
            Ok(_) => {}
            Err(Error::Api { status: 409, .. }) => {}
            Err(Error::Api { message, .. }) if message.contains("already exists") => {}
            Err(e) => return Err(e),
        }
        let mut body = json!({
            "name": req.name,
            "commercial_type": req.instance_type,
            "image": req.image,
            "project": self.project_id,
            "dynamic_ip_required": true,
        });
        if let Some(ud) = &req.user_data {
            body["user_data"] = json!({ "cloud-init": ud });
        }
        let v = self
            .request(
                reqwest::Method::POST,
                &self.instance_path("/servers"),
                Some(body),
            )
            .await?;
        let server = parse_instance(&v["server"]);
        if let Some(net) = req.network_id {
            self.request(
                reqwest::Method::POST,
                &self.instance_path(&format!("/servers/{}/private_nics", server.id)),
                Some(json!({ "private_network_id": net })),
            )
            .await?;
        }
        // Servers are created stopped; boot it.
        self.server_action(&server.id, "poweron").await?;
        self.get_instance(&server.id).await
    }

    async fn get_instance(&self, id: &str) -> Result<Instance> {
        let v = self
            .get(&self.instance_path(&format!("/servers/{id}")))
            .await?;
        if v["server"].is_null() {
            return Err(Error::NotFound {
                provider: PROVIDER,
                resource: "instance",
                id: id.to_owned(),
            });
        }
        Ok(parse_instance(&v["server"]))
    }

    async fn list_instances(&self) -> Result<Vec<Instance>> {
        let v = self
            .get(&self.instance_path("/servers?per_page=100"))
            .await?;
        Ok(v["servers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(parse_instance)
            .collect())
    }

    async fn delete_instance(&self, id: &str) -> Result<()> {
        // terminate powers off, deletes the server, and releases its volumes.
        self.server_action(id, "terminate").await
    }

    async fn create_volume(&self, req: CreateVolume) -> Result<Volume> {
        let v = self
            .request(
                reqwest::Method::POST,
                &self.instance_path("/volumes"),
                Some(json!({
                    "name": req.name,
                    "size": (req.size_gb as u64) * 1_000_000_000,
                    "volume_type": "b_ssd",
                    "project": self.project_id,
                })),
            )
            .await?;
        let vol = Volume {
            id: v["volume"]["id"].as_str().unwrap_or_default().to_owned(),
            name: v["volume"]["name"].as_str().unwrap_or_default().to_owned(),
            region: v["volume"]["zone"]
                .as_str()
                .unwrap_or(&self.zone)
                .to_owned(),
            size_gb: (v["volume"]["size"].as_u64().unwrap_or_default() / 1_000_000_000) as u32,
            attached_to: None,
        };
        if let Some(instance_id) = req.instance_id {
            self.attach_volume(&vol.id, &instance_id).await?;
            return Ok(Volume {
                attached_to: Some(instance_id),
                ..vol
            });
        }
        Ok(vol)
    }

    async fn list_volumes(&self) -> Result<Vec<Volume>> {
        let v = self
            .get(&self.instance_path("/volumes?per_page=100"))
            .await?;
        Ok(v["volumes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|vol| Volume {
                id: vol["id"].as_str().unwrap_or_default().to_owned(),
                name: vol["name"].as_str().unwrap_or_default().to_owned(),
                region: vol["zone"].as_str().unwrap_or_default().to_owned(),
                size_gb: (vol["size"].as_u64().unwrap_or_default() / 1_000_000_000) as u32,
                attached_to: vol["server"]["id"].as_str().map(str::to_owned),
            })
            .collect())
    }

    async fn attach_volume(&self, volume_id: &str, instance_id: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            &self.instance_path(&format!("/servers/{instance_id}/attach-volume")),
            Some(json!({ "volume_id": volume_id })),
        )
        .await?;
        Ok(())
    }

    async fn detach_volume(&self, volume_id: &str) -> Result<()> {
        // The detach endpoint lives on the server — look up the attachment.
        let v = self
            .get(&self.instance_path(&format!("/volumes/{volume_id}")))
            .await?;
        let Some(server_id) = v["volume"]["server"]["id"].as_str() else {
            return Ok(()); // not attached — nothing to do
        };
        self.request(
            reqwest::Method::POST,
            &self.instance_path(&format!("/servers/{server_id}/detach-volume")),
            Some(json!({ "volume_id": volume_id })),
        )
        .await?;
        Ok(())
    }

    async fn delete_volume(&self, id: &str) -> Result<()> {
        self.request(
            reqwest::Method::DELETE,
            &self.instance_path(&format!("/volumes/{id}")),
            None,
        )
        .await?;
        Ok(())
    }

    async fn create_network(&self, req: CreateNetwork) -> Result<Network> {
        let region = self.vpc_region();
        let v = self
            .request(
                reqwest::Method::POST,
                &format!("/vpc/v2/regions/{region}/private-networks"),
                Some(json!({
                    "name": req.name,
                    "project_id": self.project_id,
                    "subnets": [req.ip_range],
                })),
            )
            .await?;
        Ok(Network {
            id: v["id"].as_str().unwrap_or_default().to_owned(),
            name: v["name"].as_str().unwrap_or_default().to_owned(),
            region: v["region"].as_str().unwrap_or(&region).to_owned(),
            ip_range: req.ip_range,
        })
    }

    async fn list_networks(&self) -> Result<Vec<Network>> {
        let region = self.vpc_region();
        let v = self
            .get(&format!("/vpc/v2/regions/{region}/private-networks"))
            .await?;
        Ok(v["private_networks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|n| Network {
                id: n["id"].as_str().unwrap_or_default().to_owned(),
                name: n["name"].as_str().unwrap_or_default().to_owned(),
                region: n["region"].as_str().unwrap_or_default().to_owned(),
                ip_range: n["subnets"]
                    .as_array()
                    .and_then(|a| a.first())
                    .map(|s| {
                        s["subnet"]
                            .as_str()
                            .or_else(|| s.as_str())
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .unwrap_or_default(),
            })
            .collect())
    }

    async fn delete_network(&self, id: &str) -> Result<()> {
        let region = self.vpc_region();
        self.request(
            reqwest::Method::DELETE,
            &format!("/vpc/v2/regions/{region}/private-networks/{id}"),
            None,
        )
        .await?;
        Ok(())
    }
}
