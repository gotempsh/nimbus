//! Vultr adapter. API docs: https://www.vultr.com/api/
//! Flat REST + bearer token, no request signing.

use crate::{
    CloudProvider, CreateInstance, CreateNetwork, CreateVolume, Error, Instance, InstanceStatus,
    InstanceType, Network, Region, Result, Volume,
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

const BASE: &str = "https://api.vultr.com/v2";
const PROVIDER: &str = "vultr";

pub struct Vultr {
    token: String,
    client: Client,
}

impl Vultr {
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
        if status == StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(Error::Api { provider: PROVIDER, status: status.as_u16(), message: text });
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| Error::Api { provider: PROVIDER, status: status.as_u16(), message: e.to_string() })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, path, None).await
    }
}

fn instance_status(power: &str, status: &str) -> InstanceStatus {
    if status == "pending" {
        InstanceStatus::Provisioning
    } else if power == "running" {
        InstanceStatus::Running
    } else {
        InstanceStatus::Stopped
    }
}

fn parse_instance(v: &Value) -> Instance {
    Instance {
        id: v["id"].as_str().unwrap_or_default().to_owned(),
        name: v["label"].as_str().unwrap_or_default().to_owned(),
        region: v["region"].as_str().unwrap_or_default().to_owned(),
        instance_type: v["plan"].as_str().unwrap_or_default().to_owned(),
        status: instance_status(
            v["power_status"].as_str().unwrap_or_default(),
            v["status"].as_str().unwrap_or_default(),
        ),
        public_ipv4: v["main_ip"].as_str().filter(|s| *s != "0.0.0.0").map(str::to_owned),
        private_ipv4: v["internal_ip"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
    }
}

#[async_trait]
impl CloudProvider for Vultr {
    fn id(&self) -> &'static str {
        PROVIDER
    }

    async fn regions(&self) -> Result<Vec<Region>> {
        let v = self.get("/regions").await?;
        Ok(v["regions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|r| Region {
                id: r["id"].as_str().unwrap_or_default().to_owned(),
                name: r["city"].as_str().unwrap_or_default().to_owned(),
                country: r["country"].as_str().map(str::to_owned),
            })
            .collect())
    }

    async fn instance_types(&self, region: &str) -> Result<Vec<InstanceType>> {
        let v = self.get(&format!("/regions/{region}/availability")).await?;
        let available: Vec<String> = v["available_plans"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect();
        let plans = self.get("/plans").await?;
        Ok(plans["plans"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| {
                let id = p["id"].as_str().unwrap_or_default();
                available.is_empty() || available.iter().any(|a| a == id)
            })
            .map(|p| InstanceType {
                id: p["id"].as_str().unwrap_or_default().to_owned(),
                name: p["id"].as_str().unwrap_or_default().to_owned(),
                vcpus: p["vcpu_count"].as_u64().unwrap_or_default() as u32,
                memory_gb: p["ram"].as_f64().unwrap_or_default() as f32 / 1024.0,
                disk_gb: p["disk"].as_u64().unwrap_or_default() as u32,
                monthly_usd: p["monthly_cost"].as_f64().unwrap_or_default(),
            })
            .collect())
    }

    async fn create_instance(&self, req: CreateInstance) -> Result<Instance> {
        let key = self
            .request(
                reqwest::Method::POST,
                "/ssh-keys",
                Some(json!({ "name": format!("{}-key", req.name), "ssh_key": req.ssh_public_key })),
            )
            .await?;
        let key_id = key["ssh_key"]["id"].as_str().unwrap_or_default();
        let mut body = json!({
            "region": req.region,
            "plan": req.instance_type,
            "label": req.name,
            "os_id": 1743, // Ubuntu 24.04 LTS x64; `image` overrides via image_id below.
            "sshkey_id": [key_id],
        });
        if req.image.starts_with("iso-") || req.image.chars().all(|c| c.is_ascii_digit()) {
            body["os_id"] = json!(req.image.parse::<u64>().unwrap_or(1743));
        } else {
            body["image_id"] = json!(req.image);
            body.as_object_mut().unwrap().remove("os_id");
        }
        if let Some(ud) = req.user_data {
            body["user_data"] = json!(ud);
        }
        if let Some(net) = req.network_id {
            body["attach_vpc"] = json!([net]);
        }
        let v = self.request(reqwest::Method::POST, "/instances", Some(body)).await?;
        Ok(parse_instance(&v["instance"]))
    }

    async fn get_instance(&self, id: &str) -> Result<Instance> {
        let v = self.get(&format!("/instances/{id}")).await?;
        if v["instance"].is_null() {
            return Err(Error::NotFound { provider: PROVIDER, resource: "instance", id: id.to_owned() });
        }
        Ok(parse_instance(&v["instance"]))
    }

    async fn list_instances(&self) -> Result<Vec<Instance>> {
        let v = self.get("/instances").await?;
        Ok(v["instances"].as_array().cloned().unwrap_or_default().iter().map(parse_instance).collect())
    }

    async fn delete_instance(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/instances/{id}"), None).await?;
        Ok(())
    }

    async fn create_volume(&self, req: CreateVolume) -> Result<Volume> {
        let v = self
            .request(
                reqwest::Method::POST,
                "/blocks",
                Some(json!({ "region": req.region, "size_gb": req.size_gb, "label": req.name })),
            )
            .await?;
        let vol = Volume {
            id: v["block"]["id"].as_str().unwrap_or_default().to_owned(),
            name: v["block"]["label"].as_str().unwrap_or_default().to_owned(),
            region: v["block"]["region"].as_str().unwrap_or_default().to_owned(),
            size_gb: v["block"]["size_gb"].as_u64().unwrap_or_default() as u32,
            attached_to: None,
        };
        if let Some(instance_id) = req.instance_id {
            self.attach_volume(&vol.id, &instance_id).await?;
            return Ok(Volume { attached_to: Some(instance_id), ..vol });
        }
        Ok(vol)
    }

    async fn list_volumes(&self) -> Result<Vec<Volume>> {
        let v = self.get("/blocks").await?;
        Ok(v["blocks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|b| Volume {
                id: b["id"].as_str().unwrap_or_default().to_owned(),
                name: b["label"].as_str().unwrap_or_default().to_owned(),
                region: b["region"].as_str().unwrap_or_default().to_owned(),
                size_gb: b["size_gb"].as_u64().unwrap_or_default() as u32,
                attached_to: b["attached_to_instance"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
            })
            .collect())
    }

    async fn attach_volume(&self, volume_id: &str, instance_id: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            &format!("/blocks/{volume_id}/attach"),
            Some(json!({ "instance_id": instance_id })),
        )
        .await?;
        Ok(())
    }

    async fn detach_volume(&self, volume_id: &str) -> Result<()> {
        self.request(reqwest::Method::POST, &format!("/blocks/{volume_id}/detach"), None).await?;
        Ok(())
    }

    async fn delete_volume(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/blocks/{id}"), None).await?;
        Ok(())
    }

    async fn create_network(&self, req: CreateNetwork) -> Result<Network> {
        let v = self
            .request(
                reqwest::Method::POST,
                "/vpcs",
                Some(json!({ "region": req.region, "description": req.name, "v4_subnet_mask": 20,
                    "v4_subnet": req.ip_range.split('/').next().unwrap_or("10.0.0.0") })),
            )
            .await?;
        Ok(Network {
            id: v["vpc"]["id"].as_str().unwrap_or_default().to_owned(),
            name: v["vpc"]["description"].as_str().unwrap_or_default().to_owned(),
            region: v["vpc"]["region"].as_str().unwrap_or_default().to_owned(),
            ip_range: req.ip_range,
        })
    }

    async fn list_networks(&self) -> Result<Vec<Network>> {
        let v = self.get("/vpcs").await?;
        Ok(v["vpcs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|n| Network {
                id: n["id"].as_str().unwrap_or_default().to_owned(),
                name: n["description"].as_str().unwrap_or_default().to_owned(),
                region: n["region"].as_str().unwrap_or_default().to_owned(),
                ip_range: format!(
                    "{}/{}",
                    n["v4_subnet"].as_str().unwrap_or_default(),
                    n["v4_subnet_mask"].as_u64().unwrap_or_default()
                ),
            })
            .collect())
    }

    async fn delete_network(&self, id: &str) -> Result<()> {
        self.request(reqwest::Method::DELETE, &format!("/vpcs/{id}"), None).await?;
        Ok(())
    }
}
