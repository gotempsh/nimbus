//! DigitalOcean mock. Served under /do/v2 (Vultr owns /v2 on the shared
//! router) — point the adapter at `{root}/do/v2`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;

#[derive(Default)]
struct Store {
    next_id: AtomicU64,
    droplets: Mutex<HashMap<u64, Value>>,
    volumes: Mutex<HashMap<String, Value>>,
    vpcs: Mutex<HashMap<String, Value>>,
}

impl Store {
    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst) + 1000
    }
}

type S = State<Arc<Store>>;

pub fn router() -> Router {
    let store = Arc::new(Store::default());
    Router::new()
        .route("/do/v2/regions", get(regions))
        .route("/do/v2/sizes", get(sizes))
        .route("/do/v2/images", get(images))
        .route("/do/v2/account/keys", post(create_key).get(list_keys))
        .route("/do/v2/droplets", post(create_droplet).get(list_droplets))
        .route(
            "/do/v2/droplets/{id}",
            get(get_droplet).delete(delete_droplet),
        )
        .route("/do/v2/volumes", post(create_volume).get(list_volumes))
        .route("/do/v2/volumes/{id}", get(get_volume).delete(delete_volume))
        .route("/do/v2/volumes/{id}/actions", post(volume_action))
        .route("/do/v2/vpcs", post(create_vpc).get(list_vpcs))
        .route("/do/v2/vpcs/{id}", delete(delete_vpc))
        .with_state(store)
}

async fn regions() -> Json<Value> {
    Json(json!({ "regions": [
        { "slug": "nyc3", "name": "New York 3", "available": true },
        { "slug": "fra1", "name": "Frankfurt 1", "available": true },
    ]}))
}

async fn sizes() -> Json<Value> {
    Json(json!({ "sizes": [
        { "slug": "s-1vcpu-1gb", "description": "Basic 1GB", "memory": 1024, "vcpus": 1,
          "disk": 25, "price_monthly": 6.0, "regions": ["nyc3", "fra1"], "available": true },
    ]}))
}

async fn images() -> Json<Value> {
    Json(json!({ "images": [
        { "id": 63663980, "slug": "ubuntu-24-04-x64", "name": "Ubuntu 24.04 (LTS) x64" },
        { "id": 63663981, "slug": "debian-12-x64", "name": "Debian 12 x64" },
    ]}))
}

async fn create_key(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    Json(
        json!({ "ssh_key": { "id": store.id(), "name": body["name"], "public_key": body["public_key"] } }),
    )
}

async fn list_keys() -> Json<Value> {
    Json(json!({ "ssh_keys": [] }))
}

async fn create_droplet(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let record = json!({
        "id": id,
        "name": body["name"],
        "status": "active",
        "region": { "slug": body["region"] },
        "size_slug": body["size"],
        "networks": { "v4": [
            { "ip_address": format!("164.90.20.{}", id % 250 + 1), "type": "public" },
        ]},
    });
    store.droplets.lock().unwrap().insert(id, record.clone());
    Json(json!({ "droplet": record }))
}

async fn get_droplet(State(store): S, Path(id): Path<u64>) -> Result<Json<Value>, StatusCode> {
    store
        .droplets
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(|d| Json(json!({ "droplet": d })))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn list_droplets(State(store): S) -> Json<Value> {
    Json(
        json!({ "droplets": store.droplets.lock().unwrap().values().cloned().collect::<Vec<_>>() }),
    )
}

async fn delete_droplet(State(store): S, Path(id): Path<u64>) -> StatusCode {
    store.droplets.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}

async fn create_volume(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "id": id,
        "name": body["name"],
        "region": { "slug": body["region"] },
        "size_gigabytes": body["size_gigabytes"],
        "droplet_ids": [],
    });
    store.volumes.lock().unwrap().insert(id, record.clone());
    Json(json!({ "volume": record }))
}

async fn get_volume(State(store): S, Path(id): Path<String>) -> Result<Json<Value>, StatusCode> {
    store
        .volumes
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(|v| Json(json!({ "volume": v })))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn list_volumes(State(store): S) -> Json<Value> {
    Json(json!({ "volumes": store.volumes.lock().unwrap().values().cloned().collect::<Vec<_>>() }))
}

async fn volume_action(
    State(store): S,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    if let Some(v) = store.volumes.lock().unwrap().get_mut(&id) {
        match body["type"].as_str() {
            Some("attach") => v["droplet_ids"] = json!([body["droplet_id"]]),
            Some("detach") => v["droplet_ids"] = json!([]),
            _ => {}
        }
    }
    Json(json!({ "action": { "status": "completed" } }))
}

async fn delete_volume(State(store): S, Path(id): Path<String>) -> StatusCode {
    store.volumes.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}

async fn create_vpc(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = Uuid::new_v4().to_string();
    let record = json!({ "id": id, "name": body["name"], "region": body["region"], "ip_range": body["ip_range"] });
    store.vpcs.lock().unwrap().insert(id, record.clone());
    Json(json!({ "vpc": record }))
}

async fn list_vpcs(State(store): S) -> Json<Value> {
    Json(json!({ "vpcs": store.vpcs.lock().unwrap().values().cloned().collect::<Vec<_>>() }))
}

async fn delete_vpc(State(store): S, Path(id): Path<String>) -> StatusCode {
    store.vpcs.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}
