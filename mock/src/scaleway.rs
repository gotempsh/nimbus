//! Scaleway mock — Instance API (zone-scoped), IAM ssh-keys, VPC v2
//! private networks. Signature-free; any X-Auth-Token accepted.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Default)]
struct Store {
    servers: Mutex<HashMap<String, Value>>,
    volumes: Mutex<HashMap<String, Value>>,
    networks: Mutex<HashMap<String, Value>>,
}

type S = State<Arc<Store>>;
const P: &str = "/instance/v1/zones/{zone}";

pub fn router() -> Router {
    let store = Arc::new(Store::default());
    Router::new()
        .route(&format!("{P}/products/servers"), get(products))
        .route(&format!("{P}/images"), get(images))
        .route("/iam/v1alpha1/ssh-keys", post(create_ssh_key))
        .route(
            &format!("{P}/servers"),
            post(create_server).get(list_servers),
        )
        .route(&format!("{P}/servers/{{id}}"), get(get_server))
        .route(&format!("{P}/servers/{{id}}/action"), post(server_action))
        .route(
            &format!("{P}/servers/{{id}}/attach-volume"),
            post(attach_volume),
        )
        .route(
            &format!("{P}/servers/{{id}}/detach-volume"),
            post(detach_volume),
        )
        .route(
            &format!("{P}/servers/{{id}}/private_nics"),
            post(private_nic),
        )
        .route(
            &format!("{P}/volumes"),
            post(create_volume).get(list_volumes),
        )
        .route(
            &format!("{P}/volumes/{{id}}"),
            get(get_volume).delete(delete_volume),
        )
        .route(
            "/vpc/v2/regions/{region}/private-networks",
            post(create_network).get(list_networks),
        )
        .route(
            "/vpc/v2/regions/{region}/private-networks/{id}",
            delete(delete_network),
        )
        .with_state(store)
}

async fn products() -> Json<Value> {
    Json(json!({ "servers": {
        "DEV1-S": { "ncpus": 2, "ram": 2147483648u64, "arch": "x86_64",
                    "volumes_constraint": { "min_size": 20000000000u64 },
                    "hourly_price": 0.014, "monthly_price": 9.99 },
    }}))
}

async fn images() -> Json<Value> {
    Json(json!({ "images": [
        { "id": "11111111-2222-3333-4444-555555555555", "name": "Ubuntu 24.04 Noble Numbat" },
        { "id": "11111111-2222-3333-4444-666666666666", "name": "Debian 12 Bookworm" },
    ]}))
}

async fn create_ssh_key(Json(body): Json<Value>) -> Json<Value> {
    Json(
        json!({ "id": Uuid::new_v4().to_string(), "name": body["name"], "public_key": body["public_key"] }),
    )
}

async fn create_server(
    State(store): S,
    Path(zone): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "id": id,
        "name": body["name"],
        "commercial_type": body["commercial_type"],
        "zone": zone,
        "state": "stopped",
        "public_ip": Value::Null,
        "private_ip": Value::Null,
    });
    store.servers.lock().unwrap().insert(id, record.clone());
    Json(json!({ "server": record }))
}

async fn get_server(
    State(store): S,
    Path((_zone, id)): Path<(String, String)>,
) -> Result<Json<Value>, StatusCode> {
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

async fn server_action(
    State(store): S,
    Path((_zone, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut servers = store.servers.lock().unwrap();
    match body["action"].as_str() {
        Some("poweron") => {
            if let Some(s) = servers.get_mut(&id) {
                s["state"] = json!("running");
                s["public_ip"] = json!({ "address": "51.15.200.42" });
            }
        }
        Some("terminate") => {
            servers.remove(&id);
        }
        _ => {}
    }
    Json(json!({ "task": { "status": "pending" } }))
}

async fn private_nic(Json(_body): Json<Value>) -> Json<Value> {
    Json(json!({ "private_nic": { "id": Uuid::new_v4().to_string() } }))
}

async fn create_volume(
    State(store): S,
    Path(zone): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "id": id, "name": body["name"], "zone": zone, "size": body["size"], "server": Value::Null,
    });
    store.volumes.lock().unwrap().insert(id, record.clone());
    Json(json!({ "volume": record }))
}

async fn get_volume(
    State(store): S,
    Path((_zone, id)): Path<(String, String)>,
) -> Result<Json<Value>, StatusCode> {
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

async fn attach_volume(
    State(store): S,
    Path((_zone, server_id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    if let Some(volume_id) = body["volume_id"].as_str() {
        if let Some(v) = store.volumes.lock().unwrap().get_mut(volume_id) {
            v["server"] = json!({ "id": server_id });
        }
    }
    Json(json!({ "server": {} }))
}

async fn detach_volume(
    State(store): S,
    Path((_zone, _server_id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    if let Some(volume_id) = body["volume_id"].as_str() {
        if let Some(v) = store.volumes.lock().unwrap().get_mut(volume_id) {
            v["server"] = Value::Null;
        }
    }
    Json(json!({ "server": {} }))
}

async fn delete_volume(State(store): S, Path((_zone, id)): Path<(String, String)>) -> StatusCode {
    store.volumes.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}

async fn create_network(
    State(store): S,
    Path(region): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "id": id, "name": body["name"], "region": region, "subnets": body["subnets"],
    });
    store.networks.lock().unwrap().insert(id, record.clone());
    Json(record)
}

async fn list_networks(State(store): S) -> Json<Value> {
    Json(
        json!({ "private_networks": store.networks.lock().unwrap().values().cloned().collect::<Vec<_>>() }),
    )
}

async fn delete_network(
    State(store): S,
    Path((_region, id)): Path<(String, String)>,
) -> StatusCode {
    store.networks.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}
