//! DigitalOcean adapter. API docs: https://docs.digitalocean.com/reference/api/
//! Flat REST + bearer token, no request signing.

use crate::{
    CloudProvider, CreateInstance, CreateNetwork, CreateVolume, Error, Image, Instance,
    InstanceStatus, InstanceType, Network, Region, Result, Volume,
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

const BASE: &str = "https://api.digitalocean.com/v2";
const PROVIDER: &str = "digitalocean";

pub struct DigitalOcean {
    token: String,
    base: String,
    client: Client,
}

impl DigitalOcean {
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
}

fn instance_status(s: &str) -> InstanceStatus {
    match s {
        "active" => InstanceStatus::Running,
        "off" => InstanceStatus::Stopped,
        "new" => InstanceStatus::Provisioning,
        _ => InstanceStatus::Error,
    }
}

fn ip_of(v: &Value, ty: &str) -> Option<String> {
    v["networks"]["v4"]
        .as_array()
        .and_then(|a| a.iter().find(|n| n["type"].as_str() == Some(ty)))
        .and_then(|n| n["ip_address"].as_str())
        .map(str::to_owned)
}

fn parse_instance(v: &Value) -> Instance {
    Instance {
        id: v["id"].to_string(),
        name: v["name"].as_str().unwrap_or_default().to_owned(),
        region: v["region"]["slug"].as_str().unwrap_or_default().to_owned(),
        instance_type: v["size_slug"].as_str().unwrap_or_default().to_owned(),
        status: instance_status(v["status"].as_str().unwrap_or_default()),
        public_ipv4: ip_of(v, "public"),
        private_ipv4: ip_of(v, "private"),
        ssh_user: "root".to_owned(),
        ssh_port: 22,
    }
}

#[async_trait]
impl CloudProvider for DigitalOcean {
    fn id(&self) -> &'static str {
        PROVIDER
    }

    async fn regions(&self) -> Result<Vec<Region>> {
        let v = self.get("/regions?per_page=100").await?;
        Ok(v["regions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r["available"].as_bool().unwrap_or(true))
            .map(|r| Region {
                id: r["slug"].as_str().unwrap_or_default().to_owned(),
                name: r["name"].as_str().unwrap_or_default().to_owned(),
                country: None,
            })
            .collect())
    }

    async fn instance_types(&self, region: &str) -> Result<Vec<InstanceType>> {
        let v = self.get("/sizes?per_page=200").await?;
        Ok(v["sizes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| {
                s["available"].as_bool().unwrap_or(true)
                    && s["regions"]
                        .as_array()
                        .map(|a| a.iter().any(|r| r.as_str() == Some(region)))
                        .unwrap_or(true)
            })
            .map(|s| InstanceType {
                id: s["slug"].as_str().unwrap_or_default().to_owned(),
                name: s["description"]
                    .as_str()
                    .unwrap_or(s["slug"].as_str().unwrap_or_default())
                    .to_owned(),
                vcpus: s["vcpus"].as_u64().unwrap_or_default() as u32,
                memory_gb: s["memory"].as_f64().unwrap_or_default() as f32 / 1024.0,
                disk_gb: s["disk"].as_u64().unwrap_or_default() as u32,
                monthly_price: s["price_monthly"].as_f64().unwrap_or_default(),
                currency: "USD".to_owned(),
            })
            .collect())
    }

    async fn images(&self, _region: &str) -> Result<Vec<Image>> {
        // Distribution images are global; create accepts slug or numeric id.
        let v = self.get("/images?type=distribution&per_page=200").await?;
        Ok(v["images"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|i| Image {
                id: i["slug"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| i["id"].to_string()),
                name: i["name"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    async fn create_instance(&self, req: CreateInstance) -> Result<Instance> {
        // Upload the key; on the duplicate-key 422 fall back to the existing
        // upload instead of failing.
        let key_id = match self
            .request(
                reqwest::Method::POST,
                "/account/keys",
                Some(json!({ "name": format!("{}-key", req.name), "public_key": req.ssh_public_key })),
            )
            .await
        {
            Ok(v) => v["ssh_key"]["id"].clone(),
            Err(Error::Api { message, .. })
                if message.contains("already in use") || message.contains("duplicate") =>
            {
                let keys = self.get("/account/keys?per_page=200").await?;
                keys["ssh_keys"]
                    .as_array()
                    .and_then(|a| {
                        a.iter()
                            .find(|k| k["public_key"].as_str() == Some(req.ssh_public_key.trim()))
                    })
                    .map(|k| k["id"].clone())
                    .ok_or_else(|| {
                        Error::InvalidRequest("digitalocean: could not reuse existing ssh key".into())
                    })?
            }
            Err(e) => return Err(e),
        };
        let mut body = json!({
            "name": req.name,
            "region": req.region,
            "size": req.instance_type,
            "image": req.image,
            "ssh_keys": [key_id],
        });
        if let Some(ud) = req.user_data {
            body["user_data"] = json!(ud);
        }
        if let Some(net) = req.network_id {
            body["vpc_uuid"] = json!(net);
        }
        let v = self
            .request(reqwest::Method::POST, "/droplets", Some(body))
            .await?;
        Ok(parse_instance(&v["droplet"]))
    }

    async fn get_instance(&self, id: &str) -> Result<Instance> {
        let v = self.get(&format!("/droplets/{id}")).await?;
        if v["droplet"].is_null() {
            return Err(Error::NotFound {
                provider: PROVIDER,
                resource: "instance",
                id: id.to_owned(),
            });
        }
        Ok(parse_instance(&v["droplet"]))
    }

    async fn list_instances(&self) -> Result<Vec<Instance>> {
        let v = self.get("/droplets?per_page=200").await?;
        Ok(v["droplets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(parse_instance)
            .collect())
    }

    async fn delete_instance(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/droplets/{id}"), None)
            .await?;
        Ok(())
    }

    async fn create_volume(&self, req: CreateVolume) -> Result<Volume> {
        let v = self
            .request(
                reqwest::Method::POST,
                "/volumes",
                Some(json!({ "name": req.name, "region": req.region, "size_gigabytes": req.size_gb })),
            )
            .await?;
        let vol = Volume {
            id: v["volume"]["id"].as_str().unwrap_or_default().to_owned(),
            name: v["volume"]["name"].as_str().unwrap_or_default().to_owned(),
            region: v["volume"]["region"]["slug"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            size_gb: v["volume"]["size_gigabytes"].as_u64().unwrap_or_default() as u32,
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
        let v = self.get("/volumes?per_page=200").await?;
        Ok(v["volumes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|vol| Volume {
                id: vol["id"].as_str().unwrap_or_default().to_owned(),
                name: vol["name"].as_str().unwrap_or_default().to_owned(),
                region: vol["region"]["slug"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                size_gb: vol["size_gigabytes"].as_u64().unwrap_or_default() as u32,
                attached_to: vol["droplet_ids"]
                    .as_array()
                    .and_then(|a| a.first())
                    .map(|d| d.to_string()),
            })
            .collect())
    }

    async fn attach_volume(&self, volume_id: &str, instance_id: &str) -> Result<()> {
        let droplet: u64 = instance_id.parse().map_err(|_| {
            Error::InvalidRequest("digitalocean: instance_id must be numeric".into())
        })?;
        self.request(
            reqwest::Method::POST,
            &format!("/volumes/{volume_id}/actions"),
            Some(json!({ "type": "attach", "droplet_id": droplet })),
        )
        .await?;
        Ok(())
    }

    async fn detach_volume(&self, volume_id: &str) -> Result<()> {
        // DO's detach action requires the droplet id — look it up first.
        let v = self.get(&format!("/volumes/{volume_id}")).await?;
        let Some(droplet) = v["volume"]["droplet_ids"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_u64)
        else {
            return Ok(()); // not attached — nothing to do
        };
        self.request(
            reqwest::Method::POST,
            &format!("/volumes/{volume_id}/actions"),
            Some(json!({ "type": "detach", "droplet_id": droplet })),
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
                Some(json!({ "name": req.name, "region": req.region, "ip_range": req.ip_range })),
            )
            .await?;
        Ok(Network {
            id: v["vpc"]["id"].as_str().unwrap_or_default().to_owned(),
            name: v["vpc"]["name"].as_str().unwrap_or_default().to_owned(),
            region: v["vpc"]["region"].as_str().unwrap_or_default().to_owned(),
            ip_range: v["vpc"]["ip_range"].as_str().unwrap_or_default().to_owned(),
        })
    }

    async fn list_networks(&self) -> Result<Vec<Network>> {
        let v = self.get("/vpcs?per_page=200").await?;
        Ok(v["vpcs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|n| Network {
                id: n["id"].as_str().unwrap_or_default().to_owned(),
                name: n["name"].as_str().unwrap_or_default().to_owned(),
                region: n["region"].as_str().unwrap_or_default().to_owned(),
                ip_range: n["ip_range"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    async fn delete_network(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/vpcs/{id}"), None)
            .await?;
        Ok(())
    }
}
