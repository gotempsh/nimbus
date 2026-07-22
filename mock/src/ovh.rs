//! OVH mock. Signature headers (`X-Ovh-*`) are accepted but not verified —
//! this is a functional stand-in for the API shape, not an auth simulator.

use axum::{
    extract::{Path, Query, State},
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
    networks: Mutex<HashMap<u64, Value>>,
}

impl Store {
    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst) + 1
    }
}

type S = State<Arc<Store>>;
const P: &str = "/1.0/cloud/project/{sid}";

pub fn router() -> Router {
    let store = Arc::new(Store::default());
    Router::new()
        .route(&format!("{P}/region"), get(regions))
        .route(&format!("{P}/flavor"), get(flavors))
        .route(&format!("{P}/sshkey"), post(create_sshkey))
        .route(&format!("{P}/instance"), post(create_instance).get(list_instances))
        .route(&format!("{P}/instance/{{id}}"), get(get_instance).delete(delete_instance))
        .route(&format!("{P}/volume"), post(create_volume).get(list_volumes))
        .route(&format!("{P}/volume/{{id}}"), delete(delete_volume))
        .route(&format!("{P}/volume/{{id}}/attach"), post(attach_volume))
        .route(&format!("{P}/volume/{{id}}/detach"), post(detach_volume))
        .route(&format!("{P}/network/private"), post(create_network).get(list_networks))
        .route(&format!("{P}/network/private/{{id}}"), delete(delete_network))
        .with_state(store)
}

async fn regions() -> Json<Value> {
    Json(json!(["GRA", "SBG"]))
}

async fn flavors(Query(_params): Query<HashMap<String, String>>) -> Json<Value> {
    Json(json!([
        { "id": "d2-2", "name": "d2-2", "vcpus": 2, "ram": 4096, "disk": 50, "osType": "linux" }
    ]))
}

async fn create_sshkey(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    Json(json!({ "id": id, "name": body["name"], "publicKey": body["publicKey"] }))
}

async fn create_instance(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let record = json!({
        "id": id.to_string(),
        "name": body["name"],
        "region": body["region"],
        "flavorId": body["flavorId"],
        "status": "ACTIVE",
        "ipAddresses": [{ "type": "public", "ip": format!("51.75.{}.10", id % 254) }],
    });
    store.instances.lock().unwrap().insert(id, record.clone());
    Json(record)
}

async fn get_instance(
    State(store): S,
    Path((_sid, id)): Path<(String, u64)>,
) -> Result<Json<Value>, StatusCode> {
    store.instances.lock().unwrap().get(&id).cloned().map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn list_instances(State(store): S) -> Json<Value> {
    Json(json!(store.instances.lock().unwrap().values().cloned().collect::<Vec<_>>()))
}

async fn delete_instance(State(store): S, Path((_sid, id)): Path<(String, u64)>) -> StatusCode {
    store.instances.lock().unwrap().remove(&id);
    StatusCode::OK
}

async fn create_volume(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let record = json!({
        "id": id.to_string(), "name": body["name"], "region": body["region"], "size": body["size"],
        "attachedTo": Value::Array(vec![]),
    });
    store.volumes.lock().unwrap().insert(id, record.clone());
    Json(record)
}

async fn list_volumes(State(store): S) -> Json<Value> {
    Json(json!(store.volumes.lock().unwrap().values().cloned().collect::<Vec<_>>()))
}

async fn attach_volume(
    State(store): S,
    Path((_sid, id)): Path<(String, u64)>,
    Json(body): Json<Value>,
) -> StatusCode {
    if let Some(v) = store.volumes.lock().unwrap().get_mut(&id) {
        v["attachedTo"] = json!([body["instanceId"]]);
    }
    StatusCode::OK
}

async fn detach_volume(State(store): S, Path((_sid, id)): Path<(String, u64)>) -> StatusCode {
    if let Some(v) = store.volumes.lock().unwrap().get_mut(&id) {
        v["attachedTo"] = json!([]);
    }
    StatusCode::OK
}

async fn delete_volume(State(store): S, Path((_sid, id)): Path<(String, u64)>) -> StatusCode {
    store.volumes.lock().unwrap().remove(&id);
    StatusCode::OK
}

async fn create_network(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = store.id();
    let region = body["regions"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
    let record = json!({
        "id": id.to_string(), "name": body["name"], "regions": [{ "region": region }],
    });
    store.networks.lock().unwrap().insert(id, record.clone());
    Json(record)
}

async fn list_networks(State(store): S) -> Json<Value> {
    Json(json!(store.networks.lock().unwrap().values().cloned().collect::<Vec<_>>()))
}

async fn delete_network(State(store): S, Path((_sid, id)): Path<(String, u64)>) -> StatusCode {
    store.networks.lock().unwrap().remove(&id);
    StatusCode::OK
}
