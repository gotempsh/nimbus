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
    instances: Mutex<HashMap<String, Value>>,
    blocks: Mutex<HashMap<String, Value>>,
    vpcs: Mutex<HashMap<String, Value>>,
}

type S = State<Arc<Store>>;

pub fn router() -> Router {
    let store = Arc::new(Store::default());
    Router::new()
        .route("/v2/regions", get(regions))
        .route("/v2/regions/{id}/availability", get(availability))
        .route("/v2/plans", get(plans))
        .route("/v2/os", get(os_list))
        .route("/v2/ssh-keys", post(create_ssh_key))
        .route("/v2/instances", post(create_instance).get(list_instances))
        .route(
            "/v2/instances/{id}",
            get(get_instance).delete(delete_instance),
        )
        .route("/v2/blocks", post(create_block).get(list_blocks))
        .route("/v2/blocks/{id}", delete(delete_block))
        .route("/v2/blocks/{id}/attach", post(attach_block))
        .route("/v2/blocks/{id}/detach", post(detach_block))
        .route("/v2/vpcs", post(create_vpc).get(list_vpcs))
        .route("/v2/vpcs/{id}", delete(delete_vpc))
        .with_state(store)
}

async fn regions() -> Json<Value> {
    Json(json!({ "regions": [{ "id": "ewr", "city": "New Jersey", "country": "US" }] }))
}

async fn availability() -> Json<Value> {
    Json(json!({ "available_plans": ["vc2-1c-1gb"] }))
}

async fn plans() -> Json<Value> {
    Json(json!({ "plans": [
        { "id": "vc2-1c-1gb", "vcpu_count": 1, "ram": 1024, "disk": 25, "monthly_cost": 6.0 }
    ]}))
}

async fn os_list() -> Json<Value> {
    Json(json!({ "os": [
        { "id": 2284, "name": "Ubuntu 24.04 LTS x64" },
        { "id": 2136, "name": "Debian 12 x64" },
    ]}))
}

async fn create_ssh_key(Json(body): Json<Value>) -> Json<Value> {
    Json(
        json!({ "ssh_key": { "id": Uuid::new_v4().to_string(), "name": body["name"], "ssh_key": body["ssh_key"] } }),
    )
}

async fn create_instance(
    State(store): S,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body["plan"].as_str() != Some("vc2-1c-1gb") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid plan.", "status": 400 })),
        ));
    }
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "id": id,
        "label": body["label"],
        "region": body["region"],
        "plan": body["plan"],
        "status": "active",
        "power_status": "running",
        "main_ip": "198.51.100.10",
        "internal_ip": if body.get("attach_vpc").is_some() { "10.1.0.5" } else { "" },
    });
    store.instances.lock().unwrap().insert(id, record.clone());
    Ok(Json(json!({ "instance": record })))
}

async fn get_instance(State(store): S, Path(id): Path<String>) -> Result<Json<Value>, StatusCode> {
    store
        .instances
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(|i| Json(json!({ "instance": i })))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn list_instances(State(store): S) -> Json<Value> {
    Json(
        json!({ "instances": store.instances.lock().unwrap().values().cloned().collect::<Vec<_>>() }),
    )
}

async fn delete_instance(State(store): S, Path(id): Path<String>) -> StatusCode {
    store.instances.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}

async fn create_block(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "id": id, "label": body["label"], "region": body["region"], "size_gb": body["size_gb"],
        "attached_to_instance": "",
    });
    store.blocks.lock().unwrap().insert(id, record.clone());
    Json(json!({ "block": record }))
}

async fn list_blocks(State(store): S) -> Json<Value> {
    Json(json!({ "blocks": store.blocks.lock().unwrap().values().cloned().collect::<Vec<_>>() }))
}

async fn attach_block(
    State(store): S,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    if let Some(b) = store.blocks.lock().unwrap().get_mut(&id) {
        b["attached_to_instance"] = body["instance_id"].clone();
    }
    StatusCode::NO_CONTENT
}

async fn detach_block(State(store): S, Path(id): Path<String>) -> StatusCode {
    if let Some(b) = store.blocks.lock().unwrap().get_mut(&id) {
        b["attached_to_instance"] = json!("");
    }
    StatusCode::NO_CONTENT
}

async fn delete_block(State(store): S, Path(id): Path<String>) -> StatusCode {
    store.blocks.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}

async fn create_vpc(State(store): S, Json(body): Json<Value>) -> Json<Value> {
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "id": id, "description": body["description"], "region": body["region"],
        "v4_subnet": body["v4_subnet"], "v4_subnet_mask": body["v4_subnet_mask"],
    });
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
