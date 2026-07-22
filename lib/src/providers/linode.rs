//! Linode (Akamai) adapter. API docs: https://techdocs.akamai.com/linode-api/
//! Flat REST + bearer token, no request signing.

use crate::{
    CloudProvider, CreateInstance, CreateNetwork, CreateVolume, Error, Image, Instance,
    InstanceStatus, InstanceType, Network, Region, Result, Volume,
};
use async_trait::async_trait;
use rand::Rng;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

const BASE: &str = "https://api.linode.com/v4";
const PROVIDER: &str = "linode";

pub struct Linode {
    token: String,
    base: String,
    client: Client,
}

impl Linode {
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

fn instance_status(s: &str) -> InstanceStatus {
    match s {
        "running" => InstanceStatus::Running,
        "offline" | "stopped" => InstanceStatus::Stopped,
        "deleting" => InstanceStatus::Deleting,
        "provisioning" | "booting" | "rebooting" | "migrating" => InstanceStatus::Provisioning,
        _ => InstanceStatus::Error,
    }
}

fn parse_instance(v: &Value) -> Instance {
    let ips: Vec<&str> = v["ipv4"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    Instance {
        id: v["id"].to_string(),
        name: v["label"].as_str().unwrap_or_default().to_owned(),
        region: v["region"].as_str().unwrap_or_default().to_owned(),
        instance_type: v["type"].as_str().unwrap_or_default().to_owned(),
        status: instance_status(v["status"].as_str().unwrap_or_default()),
        public_ipv4: ips
            .iter()
            .find(|ip| !ip.starts_with("192.168.") && !ip.starts_with("10."))
            .map(|s| (*s).to_owned()),
        private_ipv4: ips
            .iter()
            .find(|ip| ip.starts_with("192.168.") || ip.starts_with("10."))
            .map(|s| (*s).to_owned()),
        ssh_user: "root".to_owned(),
        ssh_port: 22,
    }
}

/// Linode requires a root password at create time even when SSH keys are
/// provided. Generate a throwaway high-entropy one; callers log in by key.
fn random_root_pass() -> String {
    const CHARS: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#%^*-_+=";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

#[async_trait]
impl CloudProvider for Linode {
    fn id(&self) -> &'static str {
        PROVIDER
    }

    async fn regions(&self) -> Result<Vec<Region>> {
        let v = self.get("/regions").await?;
        Ok(v["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|r| Region {
                id: r["id"].as_str().unwrap_or_default().to_owned(),
                name: r["label"].as_str().unwrap_or_default().to_owned(),
                country: r["country"].as_str().map(str::to_uppercase),
            })
            .collect())
    }

    async fn instance_types(&self, _region: &str) -> Result<Vec<InstanceType>> {
        // Linode types are global; prices do not vary by region.
        let v = self.get("/linode/types").await?;
        Ok(v["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|t| InstanceType {
                id: t["id"].as_str().unwrap_or_default().to_owned(),
                name: t["label"].as_str().unwrap_or_default().to_owned(),
                vcpus: t["vcpus"].as_u64().unwrap_or_default() as u32,
                memory_gb: t["memory"].as_f64().unwrap_or_default() as f32 / 1024.0,
                disk_gb: (t["disk"].as_u64().unwrap_or_default() / 1024) as u32,
                monthly_price: t["price"]["monthly"].as_f64().unwrap_or_default(),
                currency: "USD".to_owned(),
            })
            .collect())
    }

    async fn images(&self, _region: &str) -> Result<Vec<Image>> {
        let v = self.get("/images").await?;
        Ok(v["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i["deprecated"].as_bool() != Some(true))
            .map(|i| Image {
                id: i["id"].as_str().unwrap_or_default().to_owned(),
                name: i["label"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    async fn create_instance(&self, req: CreateInstance) -> Result<Instance> {
        if req.network_id.is_some() {
            // Linode VPC attachment happens via interface configs, which
            // need a subnet id, not the VPC id — not supported yet rather
            // than silently ignored.
            return Err(Error::InvalidRequest(
                "linode: attaching a VPC at create time is not supported yet".into(),
            ));
        }
        let mut body = json!({
            "label": req.name,
            "region": req.region,
            "type": req.instance_type,
            "image": req.image,
            "root_pass": random_root_pass(),
            "authorized_keys": [req.ssh_public_key.trim()],
            "booted": true,
        });
        if let Some(ud) = req.user_data {
            // Linode metadata expects base64 user-data.
            let mut b64 = String::new();
            base64_encode(ud.as_bytes(), &mut b64);
            body["metadata"] = json!({ "user_data": b64 });
        }
        let v = self
            .request(reqwest::Method::POST, "/linode/instances", Some(body))
            .await?;
        Ok(parse_instance(&v))
    }

    async fn get_instance(&self, id: &str) -> Result<Instance> {
        let v = self.get(&format!("/linode/instances/{id}")).await?;
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
        let v = self.get("/linode/instances").await?;
        Ok(v["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(parse_instance)
            .collect())
    }

    async fn delete_instance(&self, id: &str) -> Result<()> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/linode/instances/{id}"),
            None,
        )
        .await?;
        Ok(())
    }

    async fn create_volume(&self, req: CreateVolume) -> Result<Volume> {
        let mut body = json!({ "label": req.name, "region": req.region, "size": req.size_gb });
        if let Some(sid) = &req.instance_id {
            body["linode_id"] = json!(sid.parse::<u64>().unwrap_or(0));
        }
        let v = self
            .request(reqwest::Method::POST, "/volumes", Some(body))
            .await?;
        Ok(Volume {
            id: v["id"].to_string(),
            name: v["label"].as_str().unwrap_or_default().to_owned(),
            region: v["region"].as_str().unwrap_or_default().to_owned(),
            size_gb: v["size"].as_u64().unwrap_or_default() as u32,
            attached_to: v["linode_id"].as_u64().map(|l| l.to_string()),
        })
    }

    async fn list_volumes(&self) -> Result<Vec<Volume>> {
        let v = self.get("/volumes").await?;
        Ok(v["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|vol| Volume {
                id: vol["id"].to_string(),
                name: vol["label"].as_str().unwrap_or_default().to_owned(),
                region: vol["region"].as_str().unwrap_or_default().to_owned(),
                size_gb: vol["size"].as_u64().unwrap_or_default() as u32,
                attached_to: vol["linode_id"].as_u64().map(|l| l.to_string()),
            })
            .collect())
    }

    async fn attach_volume(&self, volume_id: &str, instance_id: &str) -> Result<()> {
        let linode: u64 = instance_id
            .parse()
            .map_err(|_| Error::InvalidRequest("linode: instance_id must be numeric".into()))?;
        self.request(
            reqwest::Method::POST,
            &format!("/volumes/{volume_id}/attach"),
            Some(json!({ "linode_id": linode })),
        )
        .await?;
        Ok(())
    }

    async fn detach_volume(&self, volume_id: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            &format!("/volumes/{volume_id}/detach"),
            None,
        )
        .await?;
        Ok(())
    }

    async fn delete_volume(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/volumes/{id}"), None)
            .await?;
        Ok(())
    }

    async fn create_network(&self, req: CreateNetwork) -> Result<Network> {
        let v = self
            .request(
                reqwest::Method::POST,
                "/vpcs",
                Some(json!({
                    "label": req.name,
                    "region": req.region,
                    "subnets": [{ "label": "default", "ipv4": req.ip_range }],
                })),
            )
            .await?;
        Ok(Network {
            id: v["id"].to_string(),
            name: v["label"].as_str().unwrap_or_default().to_owned(),
            region: v["region"].as_str().unwrap_or_default().to_owned(),
            ip_range: v["subnets"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|s| s["ipv4"].as_str())
                .unwrap_or(&req.ip_range)
                .to_owned(),
        })
    }

    async fn list_networks(&self) -> Result<Vec<Network>> {
        let v = self.get("/vpcs").await?;
        Ok(v["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|n| Network {
                id: n["id"].to_string(),
                name: n["label"].as_str().unwrap_or_default().to_owned(),
                region: n["region"].as_str().unwrap_or_default().to_owned(),
                ip_range: n["subnets"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|s| s["ipv4"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect())
    }

    async fn delete_network(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/vpcs/{id}"), None)
            .await?;
        Ok(())
    }
}

/// Minimal base64 (standard alphabet, padded) to avoid another dependency.
fn base64_encode(input: &[u8], out: &mut String) {
    const TBL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TBL[(n >> 18) as usize & 63] as char);
        out.push(TBL[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TBL[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TBL[n as usize & 63] as char
        } else {
            '='
        });
    }
}
