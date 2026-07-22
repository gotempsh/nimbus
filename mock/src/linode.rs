//! Linode mock — /v4 paths, matching api.linode.com/v4 shapes.

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
    instances: Mutex<HashMap<u64, Value>>,
    volumes: Mutex<HashMap<u64, Value>>,
    vpcs: Mutex<HashMap<u64, Value>>,
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
        .route("/v4/regions", get(regions))
        .route("/v4/linode/types", get(types))
        .route("/v4/images", get(images))
        .route(
            "/v4/linode/instances",
            post(create_instance).get(list_instances),
        )
        .route(
            "/v4/linode/instances/{id}",
            get(get_instance).delete(delete_instance),
        )
        .route("/v4/volumes", post(create_volume).get(list_volumes))
        .route("/v4/volumes/{id}", delete(delete_volume))
        .route("/v4/volumes/{id}/attach", post(attach_volume))
        .route("/v4/volumes/{id}/detach", post(detach_volume))
        .route("/v4/vpcs", post(create_vpc).get(list_vpcs))
        .route("/v4/vpcs/{id}", delete(delete_vpc))
        .with_state(store)
}

async fn regions() -> Json<Value> {
    Json(json!({ "data": [
        { "id": "us-east", "label": "Newark, NJ", "country": "us" },
        { "id": "eu-central", "label": "Frankfurt, DE", "country": "de" },
    ]}))
}

async fn types() -> Json<Value> {
    Json(json!({ "data": [
        { "id": "g6-nanode-1", "label": "Nanode 1GB", "vcpus": 1, "memory": 1024,
          "disk": 25600, "price": { "monthly": 5.0 } },
    ]}))
}

async fn images() -> Json<Value> {
    Json(json!({ "data": [
        { "id": "linode/ubuntu24.04", "label": "Ubuntu 24.04 LTS", "deprecated": false },
        { "id": "linode/debian12", "label": "Debian 12", "deprecated": false },
    ]}))
}

async fn create_instance(
    State(store): S,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body["type"].as_str() != Some("g6-nanode-1") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "errors": [{ "field": "type", "reason": "type is not valid" }] })),
        ));
    }
    let id = store.id();
    let record = json!({
        "id": id,
        "label": body["label"],
        "region": body["region"],
        "type": body["type"],
        "status": "running",
        "ipv4": [format!("172.105.7.{}", id % 250 + 1), "192.168.129.10"],
    });
    store.instances.lock().unwrap().insert(id, record.clone());
    Ok(Json(record))
}

async fn get_instance(State(store): S, Path(id): Path<u64>) -> Result<Json<Value>, StatusCode> {
    store
        .instances
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn list_instances(State(store): S) -> Json<Value> {
    Json(json!({ "data": store.instances.lock().unwrap().values().cloned().collect::<Vec<_>>() }))
}

async fn delete_instance(State(store): S, Path(id): Path<u64>) -> Json<Value> {
    store.instances.lock().unwrap().remove(&id);
    Json(json!({}))
}

async fn create_volume(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let record = json!({
        "id": id, "label": body["label"], "region": body["region"], "size": body["size"],
        "linode_id": body.get("linode_id").cloned().unwrap_or(Value::Null),
    });
    store.volumes.lock().unwrap().insert(id, record.clone());
    Json(record)
}

async fn list_volumes(State(store): S) -> Json<Value> {
    Json(json!({ "data": store.volumes.lock().unwrap().values().cloned().collect::<Vec<_>>() }))
}

async fn attach_volume(
    State(store): S,
    Path(id): Path<u64>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut volumes = store.volumes.lock().unwrap();
    if let Some(v) = volumes.get_mut(&id) {
        v["linode_id"] = body["linode_id"].clone();
        return Json(v.clone());
    }
    Json(json!({}))
}

async fn detach_volume(State(store): S, Path(id): Path<u64>) -> Json<Value> {
    if let Some(v) = store.volumes.lock().unwrap().get_mut(&id) {
        v["linode_id"] = Value::Null;
    }
    Json(json!({}))
}

async fn delete_volume(State(store): S, Path(id): Path<u64>) -> Json<Value> {
    store.volumes.lock().unwrap().remove(&id);
    Json(json!({}))
}

async fn create_vpc(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let record = json!({
        "id": id, "label": body["label"], "region": body["region"], "subnets": body["subnets"],
    });
    store.vpcs.lock().unwrap().insert(id, record.clone());
    Json(record)
}

async fn list_vpcs(State(store): S) -> Json<Value> {
    Json(json!({ "data": store.vpcs.lock().unwrap().values().cloned().collect::<Vec<_>>() }))
}

async fn delete_vpc(State(store): S, Path(id): Path<u64>) -> Json<Value> {
    store.vpcs.lock().unwrap().remove(&id);
    Json(json!({}))
}
