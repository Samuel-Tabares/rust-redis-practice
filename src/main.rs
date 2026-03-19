// ================= IMPORTS =================

// Axum: framework web asíncrono
use axum::{
    extract::{Path, State}, // Permite leer parámetros de URL y estado global
    response::Html,         // Permite responder HTML
    routing::{get, post},   // Métodos HTTP
    Json, Router,           // Manejo de JSON y definición de rutas
};

// Redis en modo async (hget, hset, keys, etc.)
use redis::AsyncCommands;

// Serde: serialización y deserialización JSON <-> structs
use serde::{Deserialize, Serialize};

// Pool de conexiones a PostgreSQL (async y type‑safe)
use sqlx::PgPool;

// Arc permite compartir estado entre múltiples requests async
use std::sync::Arc;

// ================= ESTADO GLOBAL =================

/// Estado compartido de la aplicación.
///
/// Contiene las conexiones a:
/// - PostgreSQL (base de datos principal)
/// - Redis (caché)
#[derive(Clone)]
struct AppState {
    pg: PgPool,
    redis: redis::Client,
}

// ================= MODELO DE DATOS =================

/// Representa un usuario en:
/// - PostgreSQL
/// - Respuestas JSON de la API
/// - Datos almacenados en Redis
#[derive(Serialize, Deserialize, sqlx::FromRow, Clone)]
struct User {
    id: i32,
    name: String,
}

// ================= MAIN =================

/// Punto de entrada de la aplicación.
///
/// - Inicia runtime async de Tokio
/// - Conecta a Postgres y Redis
/// - Configura rutas HTTP
/// - Levanta servidor en localhost:8080
#[tokio::main]
async fn main() {
    // ---------- Conexión a PostgreSQL ----------
    let pg_pool = PgPool::connect("postgres://postgres:postgres@localhost:5432/rustdb")
        .await
        .expect("No conecta a Postgres");

    // ---------- Conexión a Redis ----------
    let redis_client = redis::Client::open("redis://127.0.0.1/")
        .expect("No conecta a Redis");

    // ---------- Estado compartido ----------
    let state = Arc::new(AppState {
        pg: pg_pool,
        redis: redis_client,
    });

    // ---------- Definición de rutas ----------
    let app = Router::new()
        .route("/", get(index))
        .route("/users", post(create_user).get(list_users_db))
        .route("/users/:id", get(get_user))
        .route("/cache", get(list_users_cache))
        .route("/cache/clear", post(clear_cache))
        .with_state(state);

    println!("Servidor en http://127.0.0.1:8080");

    // ---------- Listener TCP ----------
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .unwrap();

    // ---------- Servidor HTTP ----------
    axum::serve(listener, app).await.unwrap();
}

// ================= HTML =================

/// Devuelve una página HTML simple para probar la API desde el navegador.
async fn index() -> Html<String> {
    let html = r#"
    <html>
        <body style="font-family: sans-serif">
            <h1>Rust + Postgres + Redis</h1>

            <h2>Crear usuario</h2>
            <input id="id" placeholder="id" />
            <input id="name" placeholder="name" />
            <button onclick="createUser()">Crear</button>

            <h2>Buscar usuario (usa cache)</h2>
            <input id="searchId" placeholder="id usuario" />
            <button onclick="fetchUser()">Buscar y cachear</button>
            <pre id="userResult"></pre>

            <h2>Usuarios en PostgreSQL</h2>
            <button onclick="loadDb()">Refrescar DB</button>
            <pre id="db"></pre>

            <h2>Usuarios en Redis (cache)</h2>
            <button onclick="loadCache()">Refrescar Cache</button>
            <button onclick="clearCache()">🧹 Limpiar Cache</button>
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

/// Inserta un usuario en PostgreSQL.
async fn create_user(
    State(state): State<Arc<AppState>>, // Acceso al estado global
    Json(user): Json<User>,             // JSON recibido en el body
) -> String {
    sqlx::query("INSERT INTO users (id, name) VALUES ($1, $2)")
        .bind(user.id)
        .bind(&user.name)
        .execute(&state.pg)
        .await
        .unwrap();

    "User added to PostgreSQL".to_string()
}

// ================= GET USER (CACHE FIRST) =================

/// Obtiene un usuario:
/// 1. Busca primero en Redis (cache)
/// 2. Si no existe, consulta PostgreSQL
/// 3. Guarda resultado en Redis
async fn get_user(
    Path(id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> String {
    let key = format!("user:{id}");

    // Conexión async a Redis
    let mut redis_conn = state.redis.get_multiplexed_async_connection().await.unwrap();

    // Intentar leer desde cache
    let cached: Option<String> = redis_conn.hget(&key, "data").await.unwrap_or(None);

    // Si existe en cache → devolver inmediatamente
    if let Some(json) = cached {
        return format!("CACHE → {json}");
    }

    // Si no está en cache → consultar PostgreSQL
    let user = sqlx::query_as::<_, User>("SELECT id, name FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pg)
        .await
        .unwrap();

    // Convertir a JSON
    let json = serde_json::to_string(&user).unwrap();

    // Guardar en Redis
    let _: () = redis_conn.hset(&key, "data", &json).await.unwrap();

    format!("DB → {json}")
}

// ================= LIST USERS FROM DB =================

/// Lista todos los usuarios desde PostgreSQL.
async fn list_users_db(State(state): State<Arc<AppState>>) -> String {
    let users: Vec<User> = sqlx::query_as("SELECT id, name FROM users ORDER BY id")
        .fetch_all(&state.pg)
        .await
        .unwrap();

    serde_json::to_string_pretty(&users).unwrap()
}

// ================= LIST CACHE =================

/// Lista todos los usuarios almacenados en Redis.
async fn list_users_cache(State(state): State<Arc<AppState>>) -> String {
    let mut redis_conn = state.redis.get_multiplexed_async_connection().await.unwrap();

    // Buscar claves tipo "user:*"
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

/// Limpia completamente la base de datos de Redis.
async fn clear_cache(State(state): State<Arc<AppState>>) -> String {
    let mut redis_conn = state.redis.get_multiplexed_async_connection().await.unwrap();

    redis::cmd("FLUSHDB")
        .query_async::<_, ()>(&mut redis_conn)
        .await
        .unwrap();

    "Cache limpiado".to_string()
}