use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use fst_core::atlas::{self, MatchMode, Operation, SearchDomain};
use fst_core::journal::Journal;
use fst_core::mindmap::{self, MindmapRequest};
use fst_core::store::{Atom, SyncReport};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

type Shared = Arc<Mutex<Journal>>;

#[derive(Deserialize)]
struct Page {
    after: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct NewAtom {
    raw_content: String,
    #[serde(default)]
    methods: Vec<String>,
}

#[derive(Deserialize)]
struct AtlasNameQuery {
    name: String,
}

#[derive(Deserialize)]
struct AtlasQuery {
    names: Vec<String>,
    operation: Operation,
    #[serde(default)]
    mode: MatchMode,
    #[serde(default)]
    domain: SearchDomain,
}

// -----------------------------------------------------------------------------
// Health
// -----------------------------------------------------------------------------

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "fst-server"
    }))
}

// -----------------------------------------------------------------------------
// Atoms
// -----------------------------------------------------------------------------

async fn create_atom(
    State(db): State<Shared>,
    Json(body): Json<NewAtom>,
) -> Result<Json<Value>, StatusCode> {
    let methods: Vec<&str> = body.methods.iter().map(String::as_str).collect();

    let mut journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let id = journal
        .store
        .insert_atom(&body.raw_content, &methods)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "id": id })))
}

async fn read_atom(
    State(db): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Atom>, StatusCode> {
    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match journal.store.get_atom(&id) {
        Ok(Some(atom)) => Ok(Json(atom)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn list_atoms(
    State(db): State<Shared>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<Atom>>, StatusCode> {
    let limit = page.limit.unwrap_or(100).clamp(1, 500);

    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    journal
        .store
        .all_atoms(page.after.as_deref(), limit)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn list_by_method(
    State(db): State<Shared>,
    Path(method): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<Atom>>, StatusCode> {
    let limit = page.limit.unwrap_or(100).clamp(1, 500);

    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    journal
        .store
        .atoms_by_method(&method, page.after.as_deref(), limit)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// -----------------------------------------------------------------------------
// Methods
// -----------------------------------------------------------------------------

async fn list_methods(State(db): State<Shared>) -> Result<Json<Vec<String>>, StatusCode> {
    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    journal
        .store
        .all_methods()
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// -----------------------------------------------------------------------------
// Journal
// -----------------------------------------------------------------------------

async fn list_journal_files(State(db): State<Shared>) -> Result<Json<Vec<String>>, StatusCode> {
    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let files = journal
        .markdown_files()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(
        files
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect(),
    ))
}

async fn sync_journal(State(db): State<Shared>) -> Result<Json<SyncReport>, StatusCode> {
    let mut journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    journal
        .sync()
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// -----------------------------------------------------------------------------
// Atlas
// -----------------------------------------------------------------------------

async fn atlas_by_name(
    State(db): State<Shared>,
    Json(body): Json<AtlasNameQuery>,
) -> Result<Json<Vec<String>>, StatusCode> {
    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    atlas::hashes_by_name(&journal.store, &body.name)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn atlas_by_function(
    State(db): State<Shared>,
    Json(body): Json<AtlasNameQuery>,
) -> Result<Json<Vec<String>>, StatusCode> {
    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    atlas::hashes_by_function(&journal.store, &body.name)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn atlas_search(
    State(db): State<Shared>,
    Json(body): Json<AtlasQuery>,
) -> Result<Json<Vec<String>>, StatusCode> {
    if body.names.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    atlas::search(
        &journal.store,
        &body.names,
        body.operation,
        body.mode,
        body.domain,
    )
    .map(Json)
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

// -----------------------------------------------------------------------------
// Mindmap
// -----------------------------------------------------------------------------

async fn mindmap_json(
    State(db): State<Shared>,
    Json(body): Json<MindmapRequest>,
) -> Result<Json<mindmap::Mindmap>, StatusCode> {
    if body.query.names.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    mindmap::build(&journal.store, &body)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn mindmap_mermaid(
    State(db): State<Shared>,
    Json(body): Json<MindmapRequest>,
) -> Result<Response, StatusCode> {
    if body.query.names.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let journal = db.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let graph =
        mindmap::build(&journal.store, &body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        mindmap::to_mermaid(&graph),
    )
        .into_response())
}

async fn note_markdown(
    State(db): State<Shared>,
    Json(request): Json<MindmapRequest>,
) -> Result<Response, StatusCode> {
    if request.query.names.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let journal = db
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let graph = mindmap::build(&journal.store, &request)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let markdown = mindmap::to_markdown(&graph);

    Ok((
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        markdown,
    )
        .into_response())
}

// -----------------------------------------------------------------------------
// Server
// -----------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let root = std::env::args()
        .nth(1)
        .expect("usage: fst-server <journal-dir>");

    let journal = Journal::open(&root).expect("failed to open journal");
    let db: Shared = Arc::new(Mutex::new(journal));

    let app = Router::new()
        .route("/health", get(health))
        .route("/atoms", post(create_atom).get(list_atoms))
        .route("/atoms/all", get(list_atoms))
        .route("/atoms/{id}", get(read_atom))
        .route("/mindmap/json", post(mindmap_json))
        .route("/mindmap/mermaid", post(mindmap_mermaid))
        .route("/note/markdown", post(note_markdown))
        .route("/methods/{method}/atoms", get(list_by_method))
        .route("/methods", get(list_methods))
        .route("/journal/sync", post(sync_journal))
        .route("/journal/files", get(list_journal_files))
        .route("/atlas/name", post(atlas_by_name))
        .route("/atlas/function", post(atlas_by_function))
        .route("/atlas/search", post(atlas_search))
        .with_state(db);

    let addr = SocketAddr::from(([127, 0, 0, 1], 7878));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind server");

    println!("fst-server listening on http://{addr}");

    axum::serve(listener, app).await.expect("server failed");
}
