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

#[derive(Default)]
struct Store {
    next_id: AtomicU64,
    servers: Mutex<HashMap<u64, Value>>,
    volumes: Mutex<HashMap<u64, Value>>,
    networks: Mutex<HashMap<u64, Value>>,
}

impl Store {
    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst) + 1
    }
}

type S = State<Arc<Store>>;

pub fn router() -> Router {
    let store = Arc::new(Store::default());
    Router::new()
        .route("/v1/locations", get(locations))
        .route("/v1/server_types", get(server_types))
        .route("/v1/images", get(images))
        .route("/v1/ssh_keys", post(create_ssh_key))
        .route("/v1/servers", post(create_server).get(list_servers))
        .route("/v1/servers/{id}", get(get_server).delete(delete_server))
        .route("/v1/volumes", post(create_volume).get(list_volumes))
        .route("/v1/volumes/{id}", delete(delete_volume))
        .route("/v1/volumes/{id}/actions/attach", post(attach_volume))
        .route("/v1/volumes/{id}/actions/detach", post(detach_volume))
        .route("/v1/networks", post(create_network).get(list_networks))
        .route("/v1/networks/{id}", delete(delete_network))
        .with_state(store)
}

async fn locations() -> Json<Value> {
    Json(json!({ "locations": [
        { "name": "fsn1", "city": "Falkenstein", "country": "DE" },
        { "name": "nbg1", "city": "Nuremberg", "country": "DE" },
    ]}))
}

async fn server_types() -> Json<Value> {
    Json(json!({ "server_types": [
        {
            "name": "cx22", "description": "2 vCPU / 4GB", "cores": 2, "memory": 4.0, "disk": 40,
            "prices": [{ "location": "fsn1", "price_monthly": { "gross": "4.35" } }],
        }
    ]}))
}

async fn images() -> Json<Value> {
    Json(json!({ "images": [
        { "name": "ubuntu-24.04", "description": "Ubuntu 24.04", "deprecated": null },
        { "name": "debian-12", "description": "Debian 12", "deprecated": null },
    ]}))
}

async fn create_ssh_key(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    Json(json!({ "ssh_key": { "id": id, "name": body["name"], "public_key": body["public_key"] } }))
}

async fn create_server(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let record = json!({
        "id": id,
        "name": body["name"],
        "status": "running",
        "datacenter": { "location": { "name": body["location"] } },
        "server_type": { "name": body["server_type"] },
        "public_net": { "ipv4": { "ip": format!("203.0.113.{}", id % 254 + 1) } },
        "private_net": [],
    });
    store.servers.lock().unwrap().insert(id, record.clone());
    Json(json!({ "server": record }))
}

async fn get_server(State(store): S, Path(id): Path<u64>) -> Result<Json<Value>, StatusCode> {
    store
        .servers
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(|s| Json(json!({ "server": s })))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn list_servers(State(store): S) -> Json<Value> {
    Json(json!({ "servers": store.servers.lock().unwrap().values().cloned().collect::<Vec<_>>() }))
}

async fn delete_server(State(store): S, Path(id): Path<u64>) -> Json<Value> {
    store.servers.lock().unwrap().remove(&id);
    Json(json!({ "action": { "status": "success" } }))
}

async fn create_volume(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let record = json!({
        "id": id,
        "name": body["name"],
        "location": { "name": body["location"] },
        "size": body["size"],
        "server": body.get("server").cloned().unwrap_or(Value::Null),
    });
    store.volumes.lock().unwrap().insert(id, record.clone());
    Json(json!({ "volume": record }))
}

async fn list_volumes(State(store): S) -> Json<Value> {
    Json(json!({ "volumes": store.volumes.lock().unwrap().values().cloned().collect::<Vec<_>>() }))
}

async fn attach_volume(
    State(store): S,
    Path(id): Path<u64>,
    Json(body): Json<Value>,
) -> Json<Value> {
    if let Some(v) = store.volumes.lock().unwrap().get_mut(&id) {
        v["server"] = body["server"].clone();
    }
    Json(json!({ "action": { "status": "success" } }))
}

async fn detach_volume(State(store): S, Path(id): Path<u64>) -> Json<Value> {
    if let Some(v) = store.volumes.lock().unwrap().get_mut(&id) {
        v["server"] = Value::Null;
    }
    Json(json!({ "action": { "status": "success" } }))
}

async fn delete_volume(State(store): S, Path(id): Path<u64>) -> Json<Value> {
    store.volumes.lock().unwrap().remove(&id);
    Json(json!({ "action": { "status": "success" } }))
}

async fn create_network(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let record = json!({ "id": id, "name": body["name"], "ip_range": body["ip_range"] });
    store.networks.lock().unwrap().insert(id, record.clone());
    Json(json!({ "network": record }))
}

async fn list_networks(State(store): S) -> Json<Value> {
    Json(
        json!({ "networks": store.networks.lock().unwrap().values().cloned().collect::<Vec<_>>() }),
    )
}

async fn delete_network(State(store): S, Path(id): Path<u64>) -> Json<Value> {
    store.networks.lock().unwrap().remove(&id);
    Json(json!({ "action": { "status": "success" } }))
}
