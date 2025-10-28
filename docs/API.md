# Hanzo Node API Documentation

## Overview

Hanzo Node provides multiple API interfaces for interaction:
- **REST API (V2)**: Primary HTTP/JSON interface
- **WebSocket API**: Real-time bidirectional communication
- **Server-Sent Events (SSE)**: Streaming responses
- **Tool Execution API**: Direct tool invocation
- **LanceDB Query API**: Vector search operations

## Base URLs

- **REST API**: `http://localhost:3690/v2`
- **WebSocket**: `ws://localhost:3692`
- **Swagger UI**: `http://localhost:3690/v2/swagger-ui/`

## Authentication

### Signature-Based Authentication

All authenticated requests require an Ed25519 signature:

```http
X-Identity: <base64_public_key>
X-Signature: <base64_signature>
X-Timestamp: <unix_timestamp>
```

Example signature generation (Rust):
```rust
use ed25519_dalek::{Keypair, Signer};

let keypair = Keypair::generate(&mut OsRng);
let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
let message = format!("{}{}{}", method, path, timestamp);
let signature = keypair.sign(message.as_bytes());
```

## REST API Endpoints

### Health & Status

#### GET /v2/health
Health check endpoint.

**Response:**
```json
{
  "status": "healthy",
  "version": "1.0.0",
  "uptime_seconds": 3600
}
```

#### GET /v2/health/detailed
Detailed health status of all components.

**Response:**
```json
{
  "overall": "healthy",
  "components": {
    "database": {
      "status": "healthy",
      "latency_ms": 2
    },
    "lancedb": {
      "status": "healthy",
      "vectors_count": 15000
    },
    "job_queue": {
      "status": "healthy",
      "pending_jobs": 5,
      "processing_jobs": 2
    }
  }
}
```

### Job Management

#### POST /v2/autonomous_node
Create an autonomous job that executes with specified tools.

**Request Body:**
```json
{
  "objective": "Analyze this CSV file and create visualizations",
  "tool_names": ["read_file", "python_execute", "create_chart"],
  "identity": "user_123",
  "max_iterations": 10,
  "timeout_seconds": 300,
  "model_provider": "claude-3-opus"
}
```

**Response:**
```json
{
  "job_id": "job_abc123",
  "status": "pending",
  "created_at": "2024-01-15T10:30:00Z",
  "estimated_completion": "2024-01-15T10:35:00Z"
}
```

#### GET /v2/job/{job_id}
Get job status and results.

**Response:**
```json
{
  "job_id": "job_abc123",
  "status": "completed",
  "objective": "Analyze CSV file",
  "results": {
    "summary": "Analysis complete",
    "charts": ["chart1.png", "chart2.png"],
    "insights": ["Key finding 1", "Key finding 2"]
  },
  "tool_calls": [
    {
      "tool": "read_file",
      "duration_ms": 150,
      "status": "success"
    }
  ],
  "tokens_used": 2500,
  "cost_usd": 0.05,
  "created_at": "2024-01-15T10:30:00Z",
  "completed_at": "2024-01-15T10:33:45Z"
}
```

#### GET /v2/jobs
List all jobs with filtering options.

**Query Parameters:**
- `status`: Filter by status (pending, processing, completed, failed)
- `user`: Filter by user identity
- `limit`: Maximum results (default: 100)
- `offset`: Pagination offset

**Response:**
```json
{
  "jobs": [
    {
      "job_id": "job_abc123",
      "status": "completed",
      "objective": "Analyze data",
      "created_at": "2024-01-15T10:30:00Z"
    }
  ],
  "total": 250,
  "limit": 100,
  "offset": 0
}
```

#### DELETE /v2/job/{job_id}
Cancel a pending or running job.

**Response:**
```json
{
  "job_id": "job_abc123",
  "status": "cancelled",
  "message": "Job cancelled successfully"
}
```

### Tool Management

#### GET /v2/tools
List all available tools.

**Response:**
```json
{
  "tools": [
    {
      "name": "read_file",
      "description": "Read contents of a file",
      "parameters": {
        "type": "object",
        "properties": {
          "path": {
            "type": "string",
            "description": "File path to read"
          }
        },
        "required": ["path"]
      },
      "runtime": "native",
      "category": "file_operations"
    }
  ],
  "total": 260
}
```

#### POST /v2/tools/execute
Execute a tool directly.

**Request Body:**
```json
{
  "tool_name": "python_execute",
  "parameters": {
    "code": "import pandas as pd\ndf = pd.read_csv('data.csv')\nprint(df.head())"
  },
  "timeout": 30,
  "identity": "user_123"
}
```

**Response:**
```json
{
  "tool": "python_execute",
  "status": "success",
  "result": {
    "stdout": "   column1  column2\n0      1.0     2.0\n1      3.0     4.0",
    "stderr": "",
    "exit_code": 0
  },
  "duration_ms": 1250,
  "execution_id": "exec_xyz789"
}
```

#### GET /v2/tools/{tool_name}
Get detailed information about a specific tool.

**Response:**
```json
{
  "name": "web_search",
  "description": "Search the web using DuckDuckGo",
  "parameters": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Search query"
      },
      "max_results": {
        "type": "integer",
        "description": "Maximum number of results",
        "default": 10
      }
    },
    "required": ["query"]
  },
  "returns": {
    "type": "array",
    "items": {
      "type": "object",
      "properties": {
        "title": {"type": "string"},
        "url": {"type": "string"},
        "snippet": {"type": "string"}
      }
    }
  },
  "runtime": "deno",
  "timeout_seconds": 30,
  "rate_limit": {
    "requests_per_minute": 60
  }
}
```

### LLM Provider Management

#### GET /v2/providers
List available LLM providers and their status.

**Response:**
```json
{
  "providers": [
    {
      "name": "openai",
      "status": "active",
      "models": ["gpt-4", "gpt-3.5-turbo"],
      "capabilities": ["chat", "completion", "embeddings"],
      "rate_limits": {
        "requests_per_minute": 500,
        "tokens_per_minute": 90000
      }
    },
    {
      "name": "anthropic",
      "status": "active",
      "models": ["claude-3-opus", "claude-3-sonnet"],
      "capabilities": ["chat", "vision"],
      "rate_limits": {
        "requests_per_minute": 100,
        "tokens_per_minute": 100000
      }
    }
  ],
  "total": 45
}
```

#### POST /v2/inference
Direct LLM inference request.

**Request Body:**
```json
{
  "provider": "openai",
  "model": "gpt-4",
  "messages": [
    {
      "role": "system",
      "content": "You are a helpful assistant."
    },
    {
      "role": "user",
      "content": "Explain quantum computing in simple terms."
    }
  ],
  "temperature": 0.7,
  "max_tokens": 500,
  "stream": false
}
```

**Response:**
```json
{
  "provider": "openai",
  "model": "gpt-4",
  "response": {
    "content": "Quantum computing is...",
    "role": "assistant",
    "finish_reason": "stop"
  },
  "usage": {
    "prompt_tokens": 25,
    "completion_tokens": 150,
    "total_tokens": 175
  },
  "latency_ms": 2500
}
```

### Agent Management

#### GET /v2/agents
List configured agents.

**Response:**
```json
{
  "agents": [
    {
      "id": "agent_001",
      "name": "DataAnalyst",
      "description": "Specialized in data analysis and visualization",
      "model": "claude-3-opus",
      "tools": ["python_execute", "read_file", "create_chart"],
      "created_at": "2024-01-10T08:00:00Z"
    }
  ],
  "total": 5
}
```

#### POST /v2/agents
Create a new agent configuration.

**Request Body:**
```json
{
  "name": "CodeReviewer",
  "description": "Automated code review agent",
  "model": "gpt-4",
  "tools": ["read_file", "python_lint", "security_scan"],
  "system_prompt": "You are an expert code reviewer...",
  "parameters": {
    "temperature": 0.3,
    "max_iterations": 5
  }
}
```

**Response:**
```json
{
  "id": "agent_002",
  "name": "CodeReviewer",
  "status": "created",
  "created_at": "2024-01-15T10:45:00Z"
}
```

### Vector Operations

#### POST /v2/embeddings/generate
Generate embeddings for text.

**Request Body:**
```json
{
  "text": "Machine learning is a subset of artificial intelligence",
  "model": "qwen3-2b"
}
```

**Response:**
```json
{
  "embedding": [0.123, -0.456, 0.789, ...],
  "dimensions": 1536,
  "model": "qwen3-2b",
  "tokens": 10
}
```

#### POST /v2/embeddings/search
Search for similar vectors.

**Request Body:**
```json
{
  "query": "What is deep learning?",
  "limit": 10,
  "filters": {
    "category": "machine_learning",
    "date_after": "2024-01-01"
  },
  "hybrid": true
}
```

**Response:**
```json
{
  "results": [
    {
      "id": "doc_123",
      "score": 0.95,
      "text": "Deep learning is a subset of machine learning...",
      "metadata": {
        "source": "textbook.pdf",
        "page": 42
      }
    }
  ],
  "query_tokens": 5,
  "search_time_ms": 125
}
```

## WebSocket Protocol

### Connection

Connect to `ws://localhost:3692` with optional authentication headers.

### Message Types

#### Client → Server Messages

**Job Creation:**
```json
{
  "type": "job_create",
  "request_id": "req_123",
  "payload": {
    "objective": "Analyze data",
    "tools": ["python_execute"],
    "stream": true
  }
}
```

**Job Status Query:**
```json
{
  "type": "job_status",
  "request_id": "req_124",
  "payload": {
    "job_id": "job_abc123"
  }
}
```

**Tool Execution:**
```json
{
  "type": "tool_execute",
  "request_id": "req_125",
  "payload": {
    "tool": "web_search",
    "parameters": {
      "query": "latest AI news"
    }
  }
}
```

**Subscribe to Updates:**
```json
{
  "type": "subscribe",
  "request_id": "req_126",
  "payload": {
    "events": ["job_updates", "system_alerts"],
    "job_ids": ["job_abc123", "job_def456"]
  }
}
```

#### Server → Client Messages

**Job Update:**
```json
{
  "type": "job_update",
  "job_id": "job_abc123",
  "status": "processing",
  "progress": 0.45,
  "current_step": "Executing python code"
}
```

**Stream Chunk:**
```json
{
  "type": "stream_chunk",
  "job_id": "job_abc123",
  "chunk": "Analyzing data...",
  "index": 5,
  "finish_reason": null
}
```

**Tool Result:**
```json
{
  "type": "tool_result",
  "request_id": "req_125",
  "tool": "web_search",
  "status": "success",
  "result": {
    "results": [...]
  }
}
```

**Error Message:**
```json
{
  "type": "error",
  "request_id": "req_126",
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "Too many requests",
    "retry_after": 60
  }
}
```

### WebSocket Example (JavaScript)

```javascript
const ws = new WebSocket('ws://localhost:3692');

ws.onopen = () => {
  console.log('Connected to Hanzo Node');
  
  // Subscribe to updates
  ws.send(JSON.stringify({
    type: 'subscribe',
    request_id: 'req_001',
    payload: {
      events: ['job_updates']
    }
  }));
};

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  console.log('Received:', message);
  
  switch(message.type) {
    case 'job_update':
      console.log(`Job ${message.job_id}: ${message.status}`);
      break;
    case 'stream_chunk':
      process.stdout.write(message.chunk);
      break;
    case 'error':
      console.error('Error:', message.error);
      break;
  }
};

ws.onerror = (error) => {
  console.error('WebSocket error:', error);
};
```

## Server-Sent Events (SSE)

### Streaming Inference

**Endpoint:** `GET /v2/stream/inference`

**Request:**
```http
GET /v2/stream/inference?provider=openai&model=gpt-4
Accept: text/event-stream
```

**Response Stream:**
```
event: start
data: {"model": "gpt-4", "request_id": "req_789"}

event: chunk
data: {"content": "The answer to", "index": 0}

event: chunk
data: {"content": " your question", "index": 1}

event: done
data: {"finish_reason": "stop", "total_tokens": 150}
```

### Job Progress Stream

**Endpoint:** `GET /v2/stream/job/{job_id}`

**Response Stream:**
```
event: progress
data: {"progress": 0.25, "step": "Loading data"}

event: progress
data: {"progress": 0.50, "step": "Processing"}

event: progress
data: {"progress": 0.75, "step": "Generating output"}

event: complete
data: {"status": "completed", "results": {...}}
```

## LanceDB Query API

### Vector Search

#### POST /v2/lancedb/search
Execute vector similarity search.

**Request Body:**
```json
{
  "query_vector": [0.1, 0.2, 0.3, ...],
  "limit": 10,
  "metric": "cosine",
  "filter": {
    "category": {"$eq": "technical"},
    "date": {"$gte": "2024-01-01"}
  }
}
```

**Response:**
```json
{
  "results": [
    {
      "id": "vec_001",
      "distance": 0.95,
      "vector": [0.11, 0.21, ...],
      "metadata": {
        "text": "Original text...",
        "source": "document.pdf"
      }
    }
  ],
  "search_time_ms": 45
}
```

### Hybrid Search

#### POST /v2/lancedb/hybrid_search
Combine vector and keyword search.

**Request Body:**
```json
{
  "query": "machine learning algorithms",
  "vector_weight": 0.7,
  "keyword_weight": 0.3,
  "limit": 20,
  "rerank": true
}
```

**Response:**
```json
{
  "results": [
    {
      "id": "doc_123",
      "combined_score": 0.92,
      "vector_score": 0.95,
      "keyword_score": 0.85,
      "text": "Machine learning algorithms are...",
      "highlights": ["machine learning", "algorithms"]
    }
  ],
  "total_results": 20
}
```

## Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `INVALID_REQUEST` | 400 | Malformed request |
| `UNAUTHORIZED` | 401 | Missing or invalid authentication |
| `FORBIDDEN` | 403 | Insufficient permissions |
| `NOT_FOUND` | 404 | Resource not found |
| `RATE_LIMIT_EXCEEDED` | 429 | Too many requests |
| `INTERNAL_ERROR` | 500 | Server error |
| `SERVICE_UNAVAILABLE` | 503 | Service temporarily unavailable |

## Rate Limiting

Default rate limits per identity:
- **Requests**: 1000 per minute
- **Jobs**: 100 concurrent
- **Tools**: 500 executions per minute
- **Embeddings**: 1000 per minute

Headers returned:
```http
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 950
X-RateLimit-Reset: 1705325400
```

## Pagination

Standard pagination parameters:
- `limit`: Items per page (max: 1000)
- `offset`: Skip N items
- `cursor`: Cursor-based pagination token

Response includes:
```json
{
  "data": [...],
  "pagination": {
    "limit": 100,
    "offset": 200,
    "total": 5000,
    "has_more": true,
    "next_cursor": "cursor_abc123"
  }
}
```

## API Versioning

The API uses URL versioning:
- Current: `/v2/`
- Legacy: `/v1/` (deprecated)
- Beta: `/v3-beta/` (experimental)

## SDK Examples

### Python
```python
from hanzo import HanzoClient

client = HanzoClient(
    base_url="http://localhost:3690",
    identity="user_123"
)

# Create job
job = client.jobs.create(
    objective="Analyze sales data",
    tools=["python_execute", "create_chart"]
)

# Check status
status = client.jobs.get(job.id)
print(f"Job status: {status.status}")

# Execute tool
result = client.tools.execute(
    "web_search",
    {"query": "AI news"}
)
```

### TypeScript
```typescript
import { HanzoClient } from '@hanzo/node-sdk';

const client = new HanzoClient({
  baseUrl: 'http://localhost:3690',
  identity: 'user_123'
});

// Create job
const job = await client.jobs.create({
  objective: 'Analyze sales data',
  tools: ['python_execute', 'create_chart']
});

// Stream results
const stream = await client.jobs.stream(job.id);
for await (const chunk of stream) {
  console.log(chunk);
}
```

---

*For interactive API exploration, visit the Swagger UI at `http://localhost:3690/v2/swagger-ui/`*