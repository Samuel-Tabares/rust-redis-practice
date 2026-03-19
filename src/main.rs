// ================= IMPORTS =================

// Axum: async web framework
use axum::{
    extract::{Path, State}, // Reads URL parameters and shared state
    response::Html,         // Returns HTML responses
    routing::{get, post},   // HTTP methods
    Json, Router,           // JSON handling and route definitions
};

// Async Redis commands (hget, hset, keys, etc.)
use redis::AsyncCommands;

// Serde: JSON serialization and deserialization <-> structs
use serde::{Deserialize, Serialize};

// PostgreSQL connection pool (async and type-safe)
use sqlx::PgPool;

// Arc shares state across multiple async requests
use std::sync::Arc;

// ================= GLOBAL STATE =================

/// Shared application state.
///
/// Contains connections to:
/// - PostgreSQL (primary database)
/// - Redis (cache)
#[derive(Clone)]
struct AppState {
    pg: PgPool,
    redis: redis::Client,
}

// ================= DATA MODEL =================

/// Represents a user in:
/// - PostgreSQL
/// - API JSON responses
/// - Data stored in Redis
#[derive(Serialize, Deserialize, sqlx::FromRow, Clone)]
struct User {
    id: i32,
    name: String,
}

// ================= MAIN =================

/// Application entry point.
///
/// - Starts the Tokio async runtime
/// - Connects to Postgres and Redis
/// - Configures HTTP routes
/// - Starts the server on localhost:8080
#[tokio::main]
async fn main() {
    // ---------- PostgreSQL connection ----------
    let pg_pool = PgPool::connect("postgres://postgres:postgres@localhost:5432/rustdb")
        .await
        .expect("Failed to connect to Postgres");

    // ---------- Redis connection ----------
    let redis_client = redis::Client::open("redis://127.0.0.1/")
        .expect("Failed to connect to Redis");

    // ---------- Shared state ----------
    let state = Arc::new(AppState {
        pg: pg_pool,
        redis: redis_client,
    });

    // ---------- Route definitions ----------
    let app = Router::new()
        .route("/", get(index))
        .route("/users", post(create_user).get(list_users_db))
        .route("/users/:id", get(get_user))
        .route("/cache", get(list_users_cache))
        .route("/cache/clear", post(clear_cache))
        .with_state(state);

    println!("Server running at http://127.0.0.1:8080");

    // ---------- TCP listener ----------
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .unwrap();

    // ---------- HTTP server ----------
    axum::serve(listener, app).await.unwrap();
}

// ================= HTML =================

/// Returns a simple HTML page for testing the API in the browser.
async fn index() -> Html<String> {
    let html = r#"
    <html>
        <body style="font-family: sans-serif">
            <h1>Rust + Postgres + Redis</h1>

            <h2>Create user</h2>
            <input id="id" placeholder="id" />
            <input id="name" placeholder="name" />
            <button onclick="createUser()">Create</button>

            <h2>Find user (uses cache)</h2>
            <input id="searchId" placeholder="user id" />
            <button onclick="fetchUser()">Fetch and cache</button>
            <pre id="userResult"></pre>

            <h2>Users in PostgreSQL</h2>
            <button onclick="loadDb()">Refresh DB</button>
            <pre id="db"></pre>

            <h2>Users in Redis (cache)</h2>
            <button onclick="loadCache()">Refresh cache</button>
            <button onclick="clearCache()">Clear cache</button>
            <pre id="cache"></pre>

            <script>
                async function createUser() {
                    const id = parseInt(document.getElementById('id').value);
                    const name = document.getElementById('name').value;

                    await fetch('/users', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({ id, name })
                    });

                    loadDb();
                }

                async function fetchUser() {
                    const id = document.getElementById('searchId').value;
                    const res = await fetch(`/users/${id}`);
                    const text = await res.text();
                    document.getElementById('userResult').innerText = text;

                    loadCache();
                }

                async function loadDb() {
                    const res = await fetch('/users');
                    const data = await res.text();
                    document.getElementById('db').innerText = data;
                }

                async function loadCache() {
                    const res = await fetch('/cache');
                    const data = await res.text();
                    document.getElementById('cache').innerText = data;
                }

                async function clearCache() {
                    await fetch('/cache/clear', { method: 'POST' });
                    loadCache();
                }
            </script>
        </body>
    </html>
    "#;

    Html(html.to_string())
}

// ================= CREATE USER =================

/// Inserts a user into PostgreSQL.
async fn create_user(
    State(state): State<Arc<AppState>>, // Access to shared state
    Json(user): Json<User>,             // JSON received in the request body
) -> String {
    sqlx::query("INSERT INTO users (id, name) VALUES ($1, $2)")
        .bind(user.id)
        .bind(&user.name)
        .execute(&state.pg)
        .await
        .unwrap();

    "User inserted into PostgreSQL".to_string()
}

// ================= GET USER (CACHE FIRST) =================

/// Gets a user:
/// 1. Checks Redis first (cache)
/// 2. If missing, queries PostgreSQL
/// 3. Stores the result in Redis
async fn get_user(
    Path(id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> String {
    let key = format!("user:{id}");

    // Async Redis connection
    let mut redis_conn = state.redis.get_multiplexed_async_connection().await.unwrap();

    // Try reading from cache
    let cached: Option<String> = redis_conn.hget(&key, "data").await.unwrap_or(None);

    // If present in cache, return immediately
    if let Some(json) = cached {
        return format!("CACHE -> {json}");
    }

    // If missing from cache, query PostgreSQL
    let user = sqlx::query_as::<_, User>("SELECT id, name FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pg)
        .await
        .unwrap();

    // Convert to JSON
    let json = serde_json::to_string(&user).unwrap();

    // Store in Redis
    let _: () = redis_conn.hset(&key, "data", &json).await.unwrap();

    format!("DB -> {json}")
}

// ================= LIST USERS FROM DB =================

/// Lists all users from PostgreSQL.
async fn list_users_db(State(state): State<Arc<AppState>>) -> String {
    let users: Vec<User> = sqlx::query_as("SELECT id, name FROM users ORDER BY id")
        .fetch_all(&state.pg)
        .await
        .unwrap();

    serde_json::to_string_pretty(&users).unwrap()
}

// ================= LIST CACHE =================

/// Lists all users stored in Redis.
async fn list_users_cache(State(state): State<Arc<AppState>>) -> String {
    let mut redis_conn = state.redis.get_multiplexed_async_connection().await.unwrap();

    // Find keys matching "user:*"
    let keys: Vec<String> = redis_conn.keys("user:*").await.unwrap_or_default();

    let mut results = Vec::new();

    for key in keys {
        let data: redis::RedisResult<String> = redis_conn.hget(&key, "data").await;

        if let Ok(json) = data {
            results.push(json);
        }
    }

    serde_json::to_string_pretty(&results).unwrap()
}

// ================= CLEAR CACHE =================

/// Clears the current Redis database completely.
async fn clear_cache(State(state): State<Arc<AppState>>) -> String {
    let mut redis_conn = state.redis.get_multiplexed_async_connection().await.unwrap();

    redis::cmd("FLUSHDB")
        .query_async::<_, ()>(&mut redis_conn)
        .await
        .unwrap();

    "Cache cleared".to_string()
}
