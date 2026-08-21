//! Hostinger mock. Served at the real `/api/vps/v1`···`/api/billing/v1`
//! paths (they don't collide with any other provider's prefix). The
//! adapter's default base URL already ends in `/api`, so point
//! `with_base_url` at `{root}/api` to match.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
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
    vms: Mutex<HashMap<u64, Value>>,
}

impl Store {
    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst) + 17000
    }
}

type S = State<Arc<Store>>;

const KNOWN_ITEM_ID: &str = "hostingercom-vps-kvm2-usd-1m";

pub fn router() -> Router {
    let store = Arc::new(Store::default());
    Router::new()
        .route("/api/vps/v1/data-centers", get(data_centers))
        .route("/api/vps/v1/templates", get(templates))
        .route("/api/billing/v1/catalog", get(catalog))
        .route(
            "/api/vps/v1/virtual-machines",
            post(create_vm).get(list_vms),
        )
        .route("/api/vps/v1/virtual-machines/{id}", get(get_vm))
        .with_state(store)
}

async fn data_centers() -> Json<Value> {
    Json(json!([
        { "id": 19, "name": "phx", "location": "us", "city": "Phoenix", "continent": "North America" },
        { "id": 21, "name": "fra", "location": "de", "city": "Frankfurt", "continent": "Europe" },
    ]))
}

async fn templates() -> Json<Value> {
    Json(json!([
        { "id": 1130, "name": "Ubuntu 24.04 LTS", "description": "Ubuntu 24.04 LTS", "documentation": "https://docs.ubuntu.com" },
        { "id": 1131, "name": "Debian 12", "description": "Debian 12", "documentation": null },
    ]))
}

async fn catalog() -> Json<Value> {
    Json(json!([
        {
            "id": "hostingercom-vps-kvm2",
            "name": "KVM 2",
            "category": "VPS",
            "metadata": null,
            "prices": [
                { "id": KNOWN_ITEM_ID, "name": "KVM 2 (billed every month)", "currency": "USD",
                  "price": 799, "first_period_price": 399, "period": 1, "period_unit": "month" },
            ],
        },
    ]))
}

async fn create_vm(
    State(store): S,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body["item_id"].as_str() != Some(KNOWN_ITEM_ID) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "message": "The selected item id is invalid.",
                "errors": { "item_id": ["The selected item id is invalid."] },
            })),
        ));
    }
    let id = store.id();
    let setup = &body["setup"];
    let record = json!({
        "id": id,
        "firewall_group_id": null,
        "subscription_id": format!("sub_{id}"),
        "data_center_id": setup["data_center_id"],
        "plan": "KVM 2",
        "hostname": setup["hostname"].as_str().unwrap_or("srv.hstgr.cloud"),
        "state": "creating",
        "actions_lock": "unlocked",
        "cpus": 2,
        "memory": 8192,
        "disk": 102400,
        "bandwidth": 2147483648u64,
        "ns1": null,
        "ns2": null,
        "ipv4": [{ "id": id, "address": format!("203.0.113.{}", id % 250 + 1), "ptr": null }],
        "ipv6": null,
        "template": { "id": setup["template_id"], "name": "Ubuntu 24.04 LTS", "description": "Ubuntu 24.04 LTS", "documentation": null },
        "created_at": "2026-01-01T00:00:00.000000Z",
    });
    store.vms.lock().unwrap().insert(id, record.clone());
    Ok(Json(json!({
        "order": { "id": format!("order_{id}"), "status": "completed" },
        "virtual_machine": record,
    })))
}

async fn get_vm(State(store): S, Path(id): Path<u64>) -> Result<Json<Value>, StatusCode> {
    store
        .vms
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn list_vms(State(store): S) -> Json<Value> {
    Json(json!(store
        .vms
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect::<Vec<_>>()))
}
