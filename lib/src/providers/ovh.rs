//! OVHcloud Public Cloud adapter. API docs: https://api.ovh.com/
//! Unlike Hetzner/Vultr, OVH requires request signing (application key +
//! secret + a per-user consumer key) and scopes everything under a project
//! (`serviceName`). Endpoint is regional: eu/ca/us — default eu.

use super::Probe;
use crate::{
    Capability, CloudProvider, CreateInstance, CreateNetwork, CreateVolume, CredentialCheck, Error,
    Image, Instance, InstanceStatus, InstanceType, Network, Region, Result, Volume,
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::time::{SystemTime, UNIX_EPOCH};

const PROVIDER: &str = "ovh";

pub struct Ovh {
    endpoint: String, // e.g. "https://eu.api.ovh.com/1.0"
    application_key: String,
    application_secret: String,
    consumer_key: String,
    project_id: String,
    client: Client,
}

/// OVH API region — signing and endpoint host differ per region.
pub enum OvhRegion {
    Eu,
    Ca,
    UsWest,
}

impl OvhRegion {
    fn host(&self) -> &'static str {
        match self {
            OvhRegion::Eu => "https://eu.api.ovh.com/1.0",
            OvhRegion::Ca => "https://ca.api.ovh.com/1.0",
            OvhRegion::UsWest => "https://api.us.ovhcloud.com/1.0",
        }
    }
}

impl Ovh {
    pub fn new(
        region: OvhRegion,
        application_key: impl Into<String>,
        application_secret: impl Into<String>,
        consumer_key: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: region.host().to_owned(),
            application_key: application_key.into(),
            application_secret: application_secret.into(),
            consumer_key: consumer_key.into(),
            project_id: project_id.into(),
            client: Client::new(),
        }
    }

    /// Point at a different host — e.g. a local mock server for testing.
    /// Signing still runs against the real URL scheme, so a mock server
    /// must accept requests without verifying `X-Ovh-Signature`.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.endpoint = base.into();
        self
    }

    fn sign(&self, method: &str, url: &str, body: &str, timestamp: u64) -> String {
        // OVH signature: SHA1(AS+"+"+CK+"+"+METHOD+"+"+URL+"+"+BODY+"+"+TS)
        let payload = format!(
            "{}+{}+{method}+{url}+{body}+{timestamp}",
            self.application_secret, self.consumer_key
        );
        let mut hasher = Sha1::new();
        hasher.update(payload.as_bytes());
        format!("$1${}", hex::encode(hasher.finalize()))
    }

    /// Builds a request carrying OVH's four signing headers.
    fn signed(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.endpoint);
        let body_str = body.map(|b| b.to_string()).unwrap_or_default();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let signature = self.sign(method.as_str(), &url, &body_str, timestamp);

        let mut req = self
            .client
            .request(method, &url)
            .header("X-Ovh-Application", &self.application_key)
            .header("X-Ovh-Consumer", &self.consumer_key)
            .header("X-Ovh-Signature", signature)
            .header("X-Ovh-Timestamp", timestamp.to_string());
        if let Some(b) = body {
            req = req.json(b);
        }
        req
    }

    /// A signed GET whose status the caller inspects itself — used by
    /// `check_credential`, where a refusal is a finding rather than an error.
    async fn probe(&self, path: &str) -> Result<Probe> {
        Probe::send(self.signed(reqwest::Method::GET, path, None)).await
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let resp = self.signed(method, path, body.as_ref()).send().await?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
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

    fn project_path(&self, suffix: &str) -> String {
        format!("/cloud/project/{}{suffix}", self.project_id)
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, path, None).await
    }
}

/// Whether any of a credential's access rules (`{"method", "path"}`, where
/// `*` in a path matches any run of characters) permits `method` on `path`.
fn rules_allow(rules: &[Value], method: &str, path: &str) -> bool {
    rules.iter().any(|rule| {
        rule["method"].as_str() == Some(method)
            && rule["path"]
                .as_str()
                .is_some_and(|pattern| glob_matches(pattern, path))
    })
}

fn glob_matches(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    // `split` always yields at least one item.
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        // No `*` at all: the pattern is a literal path.
        return rest.is_empty();
    };
    for part in parts {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

fn instance_status(s: &str) -> InstanceStatus {
    match s {
        "ACTIVE" => InstanceStatus::Running,
        "SHUTOFF" | "STOPPED" => InstanceStatus::Stopped,
        "DELETING" => InstanceStatus::Deleting,
        "BUILD" | "REBUILD" => InstanceStatus::Provisioning,
        _ => InstanceStatus::Error,
    }
}

fn parse_instance(v: &Value) -> Instance {
    let ip_of = |ty: &str| {
        v["ipAddresses"]
            .as_array()
            .and_then(|a| a.iter().find(|ip| ip["type"].as_str() == Some(ty)))
            .and_then(|ip| ip["ip"].as_str())
            .map(str::to_owned)
    };
    Instance {
        id: v["id"].as_str().unwrap_or_default().to_owned(),
        name: v["name"].as_str().unwrap_or_default().to_owned(),
        region: v["region"].as_str().unwrap_or_default().to_owned(),
        instance_type: v["flavorId"].as_str().unwrap_or_default().to_owned(),
        status: instance_status(v["status"].as_str().unwrap_or_default()),
        public_ipv4: ip_of("public"),
        private_ipv4: ip_of("private"),
        // OVH Ubuntu images disable root login; the image's default sudo
        // user is "ubuntu". Other distros differ (debian, centos, ...).
        ssh_user: "ubuntu".to_owned(),
        ssh_port: 22,
    }
}

#[async_trait]
impl CloudProvider for Ovh {
    fn id(&self) -> &'static str {
        PROVIDER
    }

    async fn regions(&self) -> Result<Vec<Region>> {
        let v = self.get(&self.project_path("/region")).await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| {
                r.as_str().map(|id| Region {
                    id: id.to_owned(),
                    name: id.to_owned(),
                    country: None,
                })
            })
            .collect())
    }

    async fn instance_types(&self, region: &str) -> Result<Vec<InstanceType>> {
        let v = self
            .get(&self.project_path(&format!("/flavor?region={region}")))
            .await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f["osType"].as_str() != Some("windows"))
            .map(|f| InstanceType {
                id: f["id"].as_str().unwrap_or_default().to_owned(),
                name: f["name"].as_str().unwrap_or_default().to_owned(),
                vcpus: f["vcpus"].as_u64().unwrap_or_default() as u32,
                memory_gb: f["ram"].as_f64().unwrap_or_default() as f32 / 1024.0,
                disk_gb: f["disk"].as_u64().unwrap_or_default() as u32,
                // OVH flavor pricing needs a separate /price catalog call;
                // left at 0.0 until that's wired up (tracked follow-up).
                monthly_price: 0.0,
                currency: "EUR".to_owned(),
            })
            .collect())
    }

    async fn images(&self, region: &str) -> Result<Vec<Image>> {
        let v = self
            .get(&self.project_path(&format!("/image?region={region}&osType=linux")))
            .await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|i| Image {
                id: i["id"].as_str().unwrap_or_default().to_owned(),
                name: i["name"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    /// OVH has a real introspection endpoint: `GET /auth/currentCredential`
    /// returns the consumer key's state and the method+path rules it was
    /// granted. Creating an instance needs `POST` on the project's `/sshkey`
    /// and `/instance` paths; the project itself must exist and be `ok`.
    async fn check_credential(&self) -> Result<CredentialCheck> {
        let mut checks = vec!["GET /auth/currentCredential"];
        let credential = self.probe("/auth/currentCredential").await?;
        let rules_allow = match credential.status {
            s if s.is_success() => {
                let state = credential.body["status"].as_str().unwrap_or("unknown");
                if state != "validated" {
                    let advice = match state {
                        "pendingValidation" => "open the validation URL OVHcloud issued with the consumer key and approve it",
                        "expired" => "it has expired; request a new consumer key",
                        "refused" => "the request for it was refused; request a new consumer key",
                        _ => "request a new consumer key",
                    };
                    return Ok(CredentialCheck::rejected(
                        &checks,
                        format!("OVHcloud recognises the consumer key but it is `{state}`, not `validated`: {advice}."),
                    ));
                }
                let rules = credential.body["rules"].as_array().cloned().unwrap_or_default();
                let allowed = |path: &str| rules_allow(&rules, "POST", path);
                Some(
                    allowed(&self.project_path("/sshkey"))
                        && allowed(&self.project_path("/instance")),
                )
            }
            // A valid credential that was simply not granted this one path.
            StatusCode::FORBIDDEN
                if credential.body["errorCode"].as_str() == Some("NOT_GRANTED_CALL") =>
            {
                None
            }
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Ok(CredentialCheck::rejected(
                    &checks,
                    format!(
                        "OVHcloud rejected the credential: check that the application key, application secret and consumer key belong together and to this API endpoint (EU, CA and US accounts are separate){}.",
                        credential.reason_suffix()
                    ),
                ))
            }
            _ => return Err(credential.unexpected(PROVIDER, checks[0])),
        };

        checks.push("GET /cloud/project/{project}");
        let project = self.probe(&self.project_path("")).await?;
        let project_state = match project.status {
            s if s.is_success() => Some(
                project.body["status"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_owned(),
            ),
            StatusCode::NOT_FOUND => Some("not found".to_owned()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => None,
            _ => return Err(project.unexpected(PROVIDER, "GET /cloud/project/{project}")),
        };

        let id = &self.project_id;
        let (capability, detail) = match (rules_allow, project_state.as_deref()) {
            (Some(false), _) => (
                Capability::No,
                format!("The credential is valid but its access rules do not allow POST on /cloud/project/{id}/sshkey and /cloud/project/{id}/instance, so it cannot create instances. Request a consumer key whose rules include POST (and GET, DELETE) on /cloud/project/{id}/*."),
            ),
            (_, Some("not found")) => (
                Capability::No,
                format!("The credential is valid but Public Cloud project {id} was not found for this account. Check the project id."),
            ),
            (_, Some(state)) if state != "ok" => (
                Capability::No,
                format!("The credential is valid but Public Cloud project {id} is `{state}`, not `ok`, so instances cannot be created in it."),
            ),
            (Some(true), Some(_)) => (
                Capability::Yes,
                format!("The credential is validated, its access rules allow creating SSH keys and instances in Public Cloud project {id}, and the project is `ok`."),
            ),
            (Some(true), None) => (
                Capability::Unknown,
                format!("The credential is validated and its access rules allow creating instances in Public Cloud project {id}, but they do not allow reading the project itself, so its state could not be checked."),
            ),
            (None, _) => (
                Capability::Unknown,
                "The credential is valid but was not granted access to /auth/currentCredential, so its access rules could not be read.".to_owned(),
            ),
        };
        Ok(CredentialCheck::accepted(capability, &checks, detail))
    }

    async fn create_instance(&self, req: CreateInstance) -> Result<Instance> {
        let key = self
            .request(
                reqwest::Method::POST,
                &self.project_path("/sshkey"),
                Some(json!({ "name": format!("{}-key", req.name), "publicKey": req.ssh_public_key, "region": req.region })),
            )
            .await?;
        let mut body = json!({
            "name": req.name,
            "flavorId": req.instance_type,
            "imageId": req.image,
            "region": req.region,
            "sshKeyId": key["id"],
        });
        if let Some(ud) = req.user_data {
            body["userData"] = json!(ud);
        }
        if let Some(net) = req.network_id {
            body["networks"] = json!([{ "networkId": net }]);
        }
        let v = self
            .request(
                reqwest::Method::POST,
                &self.project_path("/instance"),
                Some(body),
            )
            .await?;
        Ok(parse_instance(&v))
    }

    async fn get_instance(&self, id: &str) -> Result<Instance> {
        let v = self
            .get(&self.project_path(&format!("/instance/{id}")))
            .await?;
        if v.is_null() {
            return Err(Error::NotFound {
                provider: PROVIDER,
                resource: "instance",
                id: id.to_owned(),
            });
        }
        Ok(parse_instance(&v))
    }

    async fn list_instances(&self) -> Result<Vec<Instance>> {
        let v = self.get(&self.project_path("/instance")).await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(parse_instance)
            .collect())
    }

    async fn delete_instance(&self, id: &str) -> Result<()> {
        self.request(
            reqwest::Method::DELETE,
            &self.project_path(&format!("/instance/{id}")),
            None,
        )
        .await?;
        Ok(())
    }

    async fn create_volume(&self, req: CreateVolume) -> Result<Volume> {
        let v = self
            .request(
                reqwest::Method::POST,
                &self.project_path("/volume"),
                Some(json!({ "name": req.name, "region": req.region, "size": req.size_gb, "type": "classic" })),
            )
            .await?;
        let vol = Volume {
            id: v["id"].as_str().unwrap_or_default().to_owned(),
            name: v["name"].as_str().unwrap_or_default().to_owned(),
            region: v["region"].as_str().unwrap_or_default().to_owned(),
            size_gb: v["size"].as_u64().unwrap_or_default() as u32,
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
        let v = self.get(&self.project_path("/volume")).await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|vol| Volume {
                id: vol["id"].as_str().unwrap_or_default().to_owned(),
                name: vol["name"].as_str().unwrap_or_default().to_owned(),
                region: vol["region"].as_str().unwrap_or_default().to_owned(),
                size_gb: vol["size"].as_u64().unwrap_or_default() as u32,
                attached_to: vol["attachedTo"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|s| s.as_str())
                    .map(str::to_owned),
            })
            .collect())
    }

    async fn attach_volume(&self, volume_id: &str, instance_id: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            &self.project_path(&format!("/volume/{volume_id}/attach")),
            Some(json!({ "instanceId": instance_id })),
        )
        .await?;
        Ok(())
    }

    async fn detach_volume(&self, volume_id: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            &self.project_path(&format!("/volume/{volume_id}/detach")),
            Some(json!({})),
        )
        .await?;
        Ok(())
    }

    async fn delete_volume(&self, id: &str) -> Result<()> {
        self.request(
            reqwest::Method::DELETE,
            &self.project_path(&format!("/volume/{id}")),
            None,
        )
        .await?;
        Ok(())
    }

    async fn create_network(&self, req: CreateNetwork) -> Result<Network> {
        let v = self
            .request(
                reqwest::Method::POST,
                &self.project_path("/network/private"),
                Some(json!({ "name": req.name, "regions": [req.region] })),
            )
            .await?;
        Ok(Network {
            id: v["id"].as_str().unwrap_or_default().to_owned(),
            name: v["name"].as_str().unwrap_or_default().to_owned(),
            region: req.region,
            ip_range: req.ip_range,
        })
    }

    async fn list_networks(&self) -> Result<Vec<Network>> {
        let v = self.get(&self.project_path("/network/private")).await?;
        Ok(v.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|n| Network {
                id: n["id"].as_str().unwrap_or_default().to_owned(),
                name: n["name"].as_str().unwrap_or_default().to_owned(),
                region: n["regions"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|r| r["region"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                ip_range: String::new(),
            })
            .collect())
    }

    async fn delete_network(&self, id: &str) -> Result<()> {
        self.request(
            reqwest::Method::DELETE,
            &self.project_path(&format!("/network/private/{id}")),
            None,
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_ovh_rule_paths() {
        for (pattern, path, expected) in [
            ("/*", "/cloud/project/p1/instance", true),
            ("/cloud/*", "/cloud/project/p1/instance", true),
            ("/cloud/project/p1/*", "/cloud/project/p1/instance", true),
            (
                "/cloud/project/*/instance",
                "/cloud/project/p1/instance",
                true,
            ),
            (
                "/cloud/project/p1/instance",
                "/cloud/project/p1/instance",
                true,
            ),
            (
                "/cloud/project/p1/instance",
                "/cloud/project/p1/instance/x",
                false,
            ),
            ("/cloud/project/p2/*", "/cloud/project/p1/instance", false),
            (
                "/cloud/project/*/volume",
                "/cloud/project/p1/instance",
                false,
            ),
            ("/domain/*", "/cloud/project/p1/instance", false),
            ("*", "/anything", true),
            ("/cloud/*/p1/*", "/cloud/project/p1/instance", true),
            (
                "/cloud/*/instance",
                "/cloud/project/p1/instance/instance",
                true,
            ),
        ] {
            assert_eq!(glob_matches(pattern, path), expected, "{pattern} vs {path}");
        }
    }

    #[test]
    fn rules_are_checked_per_method() {
        let rules = vec![
            json!({ "method": "GET", "path": "/*" }),
            json!({ "method": "POST", "path": "/cloud/project/p1/sshkey" }),
        ];
        assert!(rules_allow(&rules, "GET", "/cloud/project/p1/instance"));
        assert!(rules_allow(&rules, "POST", "/cloud/project/p1/sshkey"));
        assert!(!rules_allow(&rules, "POST", "/cloud/project/p1/instance"));
        assert!(!rules_allow(&[], "GET", "/"));
    }
}
