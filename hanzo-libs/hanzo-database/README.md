# Hanzo DB - Multi-Backend Database Abstraction

A unified database abstraction layer for Hanzo Node that supports multiple backend databases, each optimized for different workloads.

## Supported Backends

| Backend | Best For | Features |
|---------|----------|----------|
| **LanceDB** | Vector Search, AI/ML | • Native vector operations<br>• Multimodal storage<br>• Columnar format<br>• Fast similarity search |
| **DuckDB** | Analytics, OLAP | • In-process analytical database<br>• SQL support<br>• Columnar storage<br>• Fast aggregations |
| **PostgreSQL** | Transactional, OLTP | • ACID compliance<br>• Rich SQL features<br>• Extensions (pgvector)<br>• Battle-tested |
| **Redis** | Caching, Real-time | • In-memory storage<br>• Pub/sub support<br>• TTL/expiration<br>• Extremely fast |
| **SQLite** | Embedded, Lightweight | • Zero-configuration<br>• Single file<br>• Serverless<br>• Wide compatibility |

## Usage

```rust
use hanzo_db::{connect, HanzoDbConfig, DatabaseBackend, WorkloadType};

// Automatically select backend based on workload
let config = HanzoDbConfig {
    backend: DatabaseBackend::for_workload(WorkloadType::VectorSearch),
    ..Default::default()
};

// Or explicitly choose a backend
let config = HanzoDbConfig {
    backend: DatabaseBackend::LanceDB,
    path: Some("./data/vectors".into()),
    ..Default::default()
};

let db = connect(config).await?;

// Use unified interface regardless of backend
db.create_table("embeddings", schema).await?;
db.insert("embeddings", &records).await?;
let results = db.vector_search(query).await?;
```

## Features

- **Unified Interface**: Same API across all backends
- **Automatic Backend Selection**: Choose optimal backend based on workload
- **Migration Support**: Move data between backends
- **Connection Pooling**: Efficient resource management
- **Transaction Support**: ACID guarantees where supported
- **Vector Operations**: Native support in LanceDB, extension support in PostgreSQL

## Configuration

### Environment Variables

```bash
# Select default backend
HANZO_DB_BACKEND=lancedb  # lancedb, duckdb, postgresql, redis, sqlite

# Backend-specific configuration
HANZO_DB_PATH=./storage/hanzo-db
HANZO_DB_URL=postgresql://user:pass@localhost/hanzo
HANZO_REDIS_URL=redis://localhost:6379
```

### Feature Flags

```toml
[dependencies]
hanzo_db = { version = "1.0", features = ["lancedb", "duckdb", "postgres"] }
```

## Migration

Migrate from one backend to another:

```bash
# From SQLite to LanceDB
hanzo-migrate --from sqlite://old.db --to lancedb://./vectors

# From LanceDB to PostgreSQL
hanzo-migrate --from lancedb://./vectors --to postgresql://localhost/hanzo
```

## Performance Characteristics

| Operation | LanceDB | DuckDB | PostgreSQL | Redis | SQLite |
|-----------|---------|---------|------------|-------|---------|
| Vector Search | ⭐⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐ | ⭐ | ⭐ |
| Analytics | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐ | ⭐⭐ |
| Transactions | ⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐⭐ |
| Caching | ⭐⭐ | ⭐ | ⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ |
| Embedded | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐ | ⭐ | ⭐⭐⭐⭐⭐ |

## License

Apache 2.0