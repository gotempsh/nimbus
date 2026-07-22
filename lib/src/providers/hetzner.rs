//! Hetzner Cloud adapter. API docs: https://docs.hetzner.cloud/
//! Flat REST + bearer token, no request signing.

use crate::{
    CloudProvider, CreateInstance, CreateNetwork, CreateVolume, Error, Instance, InstanceStatus,
    InstanceType, Network, Region, Result, Volume,
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

const BASE: &str = "https://api.hetzner.cloud/v1";
const PROVIDER: &str = "hetzner";

pub struct Hetzner {
    token: String,
    client: Client,
}

impl Hetzner {
    pub fn new(token: impl Into<String>) -> Self {
        Self { token: token.into(), client: Client::new() }
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut req = self
            .client
            .request(method, format!("{BASE}{path}"))
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
            return Err(Error::Api { provider: PROVIDER, status: status.as_u16(), message: text });
        }
        serde_json::from_str(&text)
            .map_err(|e| Error::Api { provider: PROVIDER, status: status.as_u16(), message: e.to_string() })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, path, None).await
    }
}

fn instance_status(s: &str) -> InstanceStatus {
    match s {
        "running" => InstanceStatus::Running,
        "off" => InstanceStatus::Stopped,
        "deleting" => InstanceStatus::Deleting,
        "initializing" | "starting" | "stopping" => InstanceStatus::Provisioning,
        _ => InstanceStatus::Error,
    }
}

fn parse_instance(v: &Value) -> Instance {
    Instance {
        id: v["id"].to_string(),
        name: v["name"].as_str().unwrap_or_default().to_owned(),
        region: v["datacenter"]["location"]["name"].as_str().unwrap_or_default().to_owned(),
        instance_type: v["server_type"]["name"].as_str().unwrap_or_default().to_owned(),
        status: instance_status(v["status"].as_str().unwrap_or_default()),
        public_ipv4: v["public_net"]["ipv4"]["ip"].as_str().map(str::to_owned),
        private_ipv4: v["private_net"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|n| n["ip"].as_str())
            .map(str::to_owned),
    }
}

#[async_trait]
impl CloudProvider for Hetzner {
    fn id(&self) -> &'static str {
        PROVIDER
    }

    async fn regions(&self) -> Result<Vec<Region>> {
        let v = self.get("/locations").await?;
        Ok(v["locations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|l| Region {
                id: l["name"].as_str().unwrap_or_default().to_owned(),
                name: l["city"].as_str().unwrap_or_default().to_owned(),
                country: l["country"].as_str().map(str::to_owned),
            })
            .collect())
    }

    async fn instance_types(&self, region: &str) -> Result<Vec<InstanceType>> {
        let v = self.get("/server_types").await?;
        Ok(v["server_types"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                let price = t["prices"]
                    .as_array()?
                    .iter()
                    .find(|p| p["location"].as_str() == Some(region))
                    .or_else(|| t["prices"].as_array().and_then(|a| a.first()))?;
                let monthly_usd: f64 =
                    price["price_monthly"]["gross"].as_str().unwrap_or("0").parse().unwrap_or(0.0);
                Some(InstanceType {
                    id: t["name"].as_str().unwrap_or_default().to_owned(),
                    name: t["description"].as_str().unwrap_or_default().to_owned(),
                    vcpus: t["cores"].as_u64().unwrap_or_default() as u32,
                    memory_gb: t["memory"].as_f64().unwrap_or_default() as f32,
                    disk_gb: t["disk"].as_u64().unwrap_or_default() as u32,
                    monthly_usd,
                })
            })
            .collect())
    }

    async fn create_instance(&self, req: CreateInstance) -> Result<Instance> {
        let sshkey = self
            .request(
                reqwest::Method::POST,
                "/ssh_keys",
                Some(json!({ "name": format!("{}-key", req.name), "public_key": req.ssh_public_key })),
            )
            .await?;
        let key_id = sshkey["ssh_key"]["id"].clone();
        let mut body = json!({
            "name": req.name,
            "server_type": req.instance_type,
            "location": req.region,
            "image": req.image,
            "ssh_keys": [key_id],
        });
        if let Some(ud) = req.user_data {
            body["user_data"] = json!(ud);
        }
        if let Some(net) = req.network_id {
            body["networks"] = json!([net]);
        }
        let v = self.request(reqwest::Method::POST, "/servers", Some(body)).await?;
        Ok(parse_instance(&v["server"]))
    }

    async fn get_instance(&self, id: &str) -> Result<Instance> {
        let v = self.get(&format!("/servers/{id}")).await?;
        if v["server"].is_null() {
            return Err(Error::NotFound { provider: PROVIDER, resource: "instance", id: id.to_owned() });
        }
        Ok(parse_instance(&v["server"]))
    }

    async fn list_instances(&self) -> Result<Vec<Instance>> {
        let v = self.get("/servers").await?;
        Ok(v["servers"].as_array().cloned().unwrap_or_default().iter().map(parse_instance).collect())
    }

    async fn delete_instance(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/servers/{id}"), None).await?;
        Ok(())
    }

    async fn create_volume(&self, req: CreateVolume) -> Result<Volume> {
        let mut body = json!({ "name": req.name, "location": req.region, "size": req.size_gb });
        if let Some(sid) = &req.instance_id {
            body["server"] = json!(sid.parse::<u64>().unwrap_or(0));
        }
        let v = self.request(reqwest::Method::POST, "/volumes", Some(body)).await?;
        Ok(Volume {
            id: v["volume"]["id"].to_string(),
            name: v["volume"]["name"].as_str().unwrap_or_default().to_owned(),
            region: v["volume"]["location"]["name"].as_str().unwrap_or_default().to_owned(),
            size_gb: v["volume"]["size"].as_u64().unwrap_or_default() as u32,
            attached_to: v["volume"]["server"].as_u64().map(|s| s.to_string()),
        })
    }

    async fn list_volumes(&self) -> Result<Vec<Volume>> {
        let v = self.get("/volumes").await?;
        Ok(v["volumes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|vol| Volume {
                id: vol["id"].to_string(),
                name: vol["name"].as_str().unwrap_or_default().to_owned(),
                region: vol["location"]["name"].as_str().unwrap_or_default().to_owned(),
                size_gb: vol["size"].as_u64().unwrap_or_default() as u32,
                attached_to: vol["server"].as_u64().map(|s| s.to_string()),
            })
            .collect())
    }

    async fn attach_volume(&self, volume_id: &str, instance_id: &str) -> Result<()> {
        let server: u64 = instance_id
            .parse()
            .map_err(|_| Error::InvalidRequest("instance_id must be numeric".into()))?;
        self.request(
            reqwest::Method::POST,
            &format!("/volumes/{volume_id}/actions/attach"),
            Some(json!({ "server": server })),
        )
        .await?;
        Ok(())
    }

    async fn detach_volume(&self, volume_id: &str) -> Result<()> {
        self.request(reqwest::Method::POST, &format!("/volumes/{volume_id}/actions/detach"), None)
            .await?;
        Ok(())
    }

    async fn delete_volume(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/volumes/{id}"), None).await?;
        Ok(())
    }

    async fn create_network(&self, req: CreateNetwork) -> Result<Network> {
        let v = self
            .request(
                reqwest::Method::POST,
                "/networks",
                Some(json!({ "name": req.name, "ip_range": req.ip_range })),
            )
            .await?;
        Ok(Network {
            id: v["network"]["id"].to_string(),
            name: v["network"]["name"].as_str().unwrap_or_default().to_owned(),
            region: req.region,
            ip_range: v["network"]["ip_range"].as_str().unwrap_or_default().to_owned(),
        })
    }

    async fn list_networks(&self) -> Result<Vec<Network>> {
        let v = self.get("/networks").await?;
        Ok(v["networks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|n| Network {
                id: n["id"].to_string(),
                name: n["name"].as_str().unwrap_or_default().to_owned(),
                region: String::new(),
                ip_range: n["ip_range"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    async fn delete_network(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/networks/{id}"), None).await?;
        Ok(())
    }
}
