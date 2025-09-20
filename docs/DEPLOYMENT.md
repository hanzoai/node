# Hanzo Node Deployment Guide

## Table of Contents

1. [Production Requirements](#production-requirements)
2. [Deployment Methods](#deployment-methods)
3. [Docker Deployment](#docker-deployment)
4. [Kubernetes Deployment](#kubernetes-deployment)
5. [Bare Metal Deployment](#bare-metal-deployment)
6. [Cloud Provider Deployment](#cloud-provider-deployment)
7. [Configuration Management](#configuration-management)
8. [Monitoring Setup](#monitoring-setup)
9. [Security Hardening](#security-hardening)
10. [Backup & Recovery](#backup--recovery)
11. [Scaling Strategies](#scaling-strategies)
12. [Troubleshooting](#troubleshooting)

## Production Requirements

### Hardware Requirements

**Minimum (Single Node):**
- CPU: 4 cores (x86_64 or ARM64)
- RAM: 8GB
- Storage: 100GB SSD
- Network: 1Gbps

**Recommended (Production):**
- CPU: 16 cores
- RAM: 32GB
- Storage: 500GB NVMe SSD
- Network: 10Gbps
- GPU: Optional (NVIDIA for CUDA acceleration)

### Software Requirements

- **OS**: Ubuntu 22.04 LTS, RHEL 8+, or Alpine Linux
- **Runtime**: Docker 24+ or containerd 1.7+
- **Orchestration**: Kubernetes 1.28+ (optional)
- **Database**: SQLite 3.40+ (included)
- **TLS**: OpenSSL 3.0+

### Network Requirements

**Ports:**
| Port | Protocol | Service | Direction |
|------|----------|---------|-----------|
| 3690 | TCP | REST API | Inbound |
| 3691 | TCP | P2P | Inbound/Outbound |
| 3692 | TCP | WebSocket | Inbound |
| 9090 | TCP | Prometheus | Internal |
| 11434 | TCP | Ollama | Internal |

## Deployment Methods

### Quick Decision Matrix

| Method | Best For | Complexity | Scalability |
|--------|----------|------------|-------------|
| Docker | Single instance | Low | Vertical |
| Kubernetes | Large scale | High | Horizontal |
| Bare Metal | Maximum performance | Medium | Limited |
| Cloud | Managed infrastructure | Low | Elastic |

## Docker Deployment

### 1. Build Production Image

```dockerfile
# Dockerfile
FROM rust:1.75 AS builder

WORKDIR /app
COPY . .
RUN cargo build --release --bin hanzod

FROM ubuntu:22.04

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/hanzod /usr/local/bin/
COPY --from=builder /app/hanzo-non-rust-code /opt/hanzo/non-rust-code

ENV RUST_LOG=info
ENV NODE_IP=0.0.0.0
ENV NODE_API_PORT=3690

EXPOSE 3690 3691 3692

VOLUME ["/data"]
CMD ["hanzod"]
```

### 2. Docker Compose Configuration

```yaml
# docker-compose.yml
version: '3.8'

services:
  hanzo-node:
    image: hanzoai/hanzo-node:latest
    container_name: hanzo-node
    restart: unless-stopped
    ports:
      - "3690:3690"
      - "3691:3691"
      - "3692:3692"
    volumes:
      - ./data:/data
      - ./config:/config
    environment:
      - NODE_API_PORT=3690
      - NODE_PORT=3691
      - NODE_WS_PORT=3692
      - DATABASE_URL=/data/db.sqlite
      - LANCEDB_PATH=/data/lancedb
      - RUST_LOG=info
      - OPENAI_API_KEY=${OPENAI_API_KEY}
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3690/v2/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
    networks:
      - hanzo-network

  ollama:
    image: ollama/ollama:latest
    container_name: ollama
    restart: unless-stopped
    ports:
      - "11434:11434"
    volumes:
      - ollama-data:/root/.ollama
    environment:
      - OLLAMA_HOST=0.0.0.0
    networks:
      - hanzo-network

  prometheus:
    image: prom/prometheus:latest
    container_name: prometheus
    restart: unless-stopped
    ports:
      - "9090:9090"
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml
      - prometheus-data:/prometheus
    command:
      - '--config.file=/etc/prometheus/prometheus.yml'
      - '--storage.tsdb.path=/prometheus'
    networks:
      - hanzo-network

  grafana:
    image: grafana/grafana:latest
    container_name: grafana
    restart: unless-stopped
    ports:
      - "3000:3000"
    volumes:
      - grafana-data:/var/lib/grafana
      - ./grafana/dashboards:/etc/grafana/provisioning/dashboards
      - ./grafana/datasources:/etc/grafana/provisioning/datasources
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=${GRAFANA_PASSWORD:-admin}
      - GF_INSTALL_PLUGINS=redis-datasource
    networks:
      - hanzo-network

networks:
  hanzo-network:
    driver: bridge

volumes:
  ollama-data:
  prometheus-data:
  grafana-data:
```

### 3. Deploy with Docker

```bash
# Pull or build image
docker pull hanzoai/hanzo-node:latest
# OR
docker build -t hanzo-node:latest .

# Start services
docker compose up -d

# Check status
docker compose ps

# View logs
docker compose logs -f hanzo-node

# Scale horizontally (requires load balancer)
docker compose up -d --scale hanzo-node=3
```

## Kubernetes Deployment

### 1. Namespace and ConfigMap

```yaml
# namespace.yaml
apiVersion: v1
kind: Namespace
metadata:
  name: hanzo-system

---
# configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: hanzo-config
  namespace: hanzo-system
data:
  NODE_API_PORT: "3690"
  NODE_PORT: "3691"
  NODE_WS_PORT: "3692"
  RUST_LOG: "info"
  USE_NATIVE_EMBEDDINGS: "true"
  EMBEDDINGS_SERVER_URL: "http://ollama:11434"
```

### 2. Secret Management

```yaml
# secret.yaml
apiVersion: v1
kind: Secret
metadata:
  name: hanzo-secrets
  namespace: hanzo-system
type: Opaque
stringData:
  OPENAI_API_KEY: "sk-..."
  ANTHROPIC_API_KEY: "sk-ant-..."
  DATABASE_ENCRYPTION_KEY: "base64_encoded_key"
```

### 3. StatefulSet Deployment

```yaml
# statefulset.yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: hanzo-node
  namespace: hanzo-system
spec:
  serviceName: hanzo-node
  replicas: 3
  selector:
    matchLabels:
      app: hanzo-node
  template:
    metadata:
      labels:
        app: hanzo-node
    spec:
      containers:
      - name: hanzo-node
        image: hanzoai/hanzo-node:latest
        ports:
        - containerPort: 3690
          name: api
        - containerPort: 3691
          name: p2p
        - containerPort: 3692
          name: websocket
        envFrom:
        - configMapRef:
            name: hanzo-config
        - secretRef:
            name: hanzo-secrets
        volumeMounts:
        - name: data
          mountPath: /data
        - name: config
          mountPath: /config
        resources:
          requests:
            memory: "8Gi"
            cpu: "2"
          limits:
            memory: "16Gi"
            cpu: "4"
        livenessProbe:
          httpGet:
            path: /v2/health
            port: 3690
          initialDelaySeconds: 30
          periodSeconds: 30
        readinessProbe:
          httpGet:
            path: /v2/health
            port: 3690
          initialDelaySeconds: 10
          periodSeconds: 10
  volumeClaimTemplates:
  - metadata:
      name: data
    spec:
      accessModes: ["ReadWriteOnce"]
      storageClassName: "fast-ssd"
      resources:
        requests:
          storage: 100Gi
```

### 4. Service Configuration

```yaml
# service.yaml
apiVersion: v1
kind: Service
metadata:
  name: hanzo-node
  namespace: hanzo-system
spec:
  selector:
    app: hanzo-node
  ports:
  - name: api
    port: 3690
    targetPort: 3690
  - name: p2p
    port: 3691
    targetPort: 3691
  - name: websocket
    port: 3692
    targetPort: 3692
  type: LoadBalancer

---
# headless service for StatefulSet
apiVersion: v1
kind: Service
metadata:
  name: hanzo-node-headless
  namespace: hanzo-system
spec:
  clusterIP: None
  selector:
    app: hanzo-node
  ports:
  - name: api
    port: 3690
  - name: p2p
    port: 3691
```

### 5. Ingress Configuration

```yaml
# ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: hanzo-ingress
  namespace: hanzo-system
  annotations:
    nginx.ingress.kubernetes.io/rewrite-target: /
    cert-manager.io/cluster-issuer: "letsencrypt-prod"
    nginx.ingress.kubernetes.io/websocket-services: "hanzo-node"
spec:
  tls:
  - hosts:
    - api.hanzo.ai
    secretName: hanzo-tls
  rules:
  - host: api.hanzo.ai
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: hanzo-node
            port:
              number: 3690
```

### 6. HorizontalPodAutoscaler

```yaml
# hpa.yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: hanzo-node-hpa
  namespace: hanzo-system
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: StatefulSet
    name: hanzo-node
  minReplicas: 3
  maxReplicas: 10
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
  - type: Resource
    resource:
      name: memory
      target:
        type: Utilization
        averageUtilization: 80
  behavior:
    scaleUp:
      stabilizationWindowSeconds: 60
      policies:
      - type: Percent
        value: 100
        periodSeconds: 60
    scaleDown:
      stabilizationWindowSeconds: 300
      policies:
      - type: Percent
        value: 50
        periodSeconds: 60
```

### 7. Deploy to Kubernetes

```bash
# Create namespace and configs
kubectl apply -f namespace.yaml
kubectl apply -f configmap.yaml
kubectl apply -f secret.yaml

# Deploy application
kubectl apply -f statefulset.yaml
kubectl apply -f service.yaml
kubectl apply -f ingress.yaml
kubectl apply -f hpa.yaml

# Check deployment status
kubectl get pods -n hanzo-system
kubectl get svc -n hanzo-system
kubectl get ingress -n hanzo-system

# View logs
kubectl logs -f statefulset/hanzo-node -n hanzo-system

# Scale manually
kubectl scale statefulset hanzo-node --replicas=5 -n hanzo-system
```

## Bare Metal Deployment

### 1. System Preparation

```bash
# Update system
sudo apt update && sudo apt upgrade -y

# Install dependencies
sudo apt install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    curl \
    git \
    supervisor \
    nginx \
    prometheus \
    grafana

# Create hanzo user
sudo useradd -r -s /bin/bash -d /opt/hanzo hanzo
sudo mkdir -p /opt/hanzo
sudo chown -R hanzo:hanzo /opt/hanzo
```

### 2. Install Hanzo Node

```bash
# As hanzo user
sudo -u hanzo -i

# Clone and build
cd /opt/hanzo
git clone https://github.com/hanzoai/hanzo-node
cd hanzo-node
cargo build --release --bin hanzod

# Copy binary
cp target/release/hanzod /opt/hanzo/bin/
cp -r hanzo-non-rust-code /opt/hanzo/

# Create directories
mkdir -p /opt/hanzo/{data,logs,config}
```

### 3. Systemd Service

```ini
# /etc/systemd/system/hanzo-node.service
[Unit]
Description=Hanzo Node
After=network.target
Wants=network-online.target

[Service]
Type=simple
User=hanzo
Group=hanzo
WorkingDirectory=/opt/hanzo
Environment="RUST_LOG=info"
Environment="NODE_API_PORT=3690"
Environment="NODE_PORT=3691"
Environment="NODE_WS_PORT=3692"
Environment="DATABASE_URL=/opt/hanzo/data/db.sqlite"
Environment="LANCEDB_PATH=/opt/hanzo/data/lancedb"
EnvironmentFile=-/opt/hanzo/config/hanzo.env
ExecStart=/opt/hanzo/bin/hanzod
ExecReload=/bin/kill -HUP $MAINPID
Restart=always
RestartSec=10
StandardOutput=append:/opt/hanzo/logs/hanzo.log
StandardError=append:/opt/hanzo/logs/hanzo-error.log

# Security
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/hanzo/data /opt/hanzo/logs

[Install]
WantedBy=multi-user.target
```

### 4. Nginx Reverse Proxy

```nginx
# /etc/nginx/sites-available/hanzo
upstream hanzo_api {
    least_conn;
    server 127.0.0.1:3690;
    # Add more servers for load balancing
    # server 127.0.0.1:3790;
    # server 127.0.0.1:3890;
}

upstream hanzo_ws {
    ip_hash;
    server 127.0.0.1:3692;
}

server {
    listen 80;
    server_name api.hanzo.ai;
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    server_name api.hanzo.ai;

    ssl_certificate /etc/letsencrypt/live/api.hanzo.ai/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/api.hanzo.ai/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers HIGH:!aNULL:!MD5;

    # API endpoints
    location /v2/ {
        proxy_pass http://hanzo_api;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        
        # Timeouts for long-running requests
        proxy_read_timeout 300s;
        proxy_connect_timeout 75s;
    }

    # WebSocket endpoint
    location /ws {
        proxy_pass http://hanzo_ws;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }

    # SSE endpoint
    location /v2/stream/ {
        proxy_pass http://hanzo_api;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_buffering off;
        proxy_cache off;
        proxy_set_header Connection '';
        proxy_http_version 1.1;
        chunked_transfer_encoding off;
    }
}
```

### 5. Start Services

```bash
# Enable and start service
sudo systemctl daemon-reload
sudo systemctl enable hanzo-node
sudo systemctl start hanzo-node

# Check status
sudo systemctl status hanzo-node

# View logs
sudo journalctl -u hanzo-node -f

# Restart nginx
sudo nginx -t
sudo systemctl reload nginx
```

## Cloud Provider Deployment

### AWS ECS Deployment

```yaml
# task-definition.json
{
  "family": "hanzo-node",
  "taskRoleArn": "arn:aws:iam::123456789012:role/hanzo-task-role",
  "executionRoleArn": "arn:aws:iam::123456789012:role/hanzo-execution-role",
  "networkMode": "awsvpc",
  "requiresCompatibilities": ["FARGATE"],
  "cpu": "4096",
  "memory": "16384",
  "containerDefinitions": [
    {
      "name": "hanzo-node",
      "image": "hanzoai/hanzo-node:latest",
      "portMappings": [
        {"containerPort": 3690, "protocol": "tcp"},
        {"containerPort": 3691, "protocol": "tcp"},
        {"containerPort": 3692, "protocol": "tcp"}
      ],
      "environment": [
        {"name": "NODE_API_PORT", "value": "3690"},
        {"name": "RUST_LOG", "value": "info"}
      ],
      "secrets": [
        {
          "name": "OPENAI_API_KEY",
          "valueFrom": "arn:aws:secretsmanager:us-west-2:123456789012:secret:hanzo/openai"
        }
      ],
      "mountPoints": [
        {
          "sourceVolume": "hanzo-data",
          "containerPath": "/data"
        }
      ],
      "logConfiguration": {
        "logDriver": "awslogs",
        "options": {
          "awslogs-group": "/ecs/hanzo-node",
          "awslogs-region": "us-west-2",
          "awslogs-stream-prefix": "ecs"
        }
      },
      "healthCheck": {
        "command": ["CMD-SHELL", "curl -f http://localhost:3690/v2/health || exit 1"],
        "interval": 30,
        "timeout": 5,
        "retries": 3,
        "startPeriod": 60
      }
    }
  ],
  "volumes": [
    {
      "name": "hanzo-data",
      "efsVolumeConfiguration": {
        "fileSystemId": "fs-12345678",
        "rootDirectory": "/hanzo"
      }
    }
  ]
}
```

### Google Cloud Run

```yaml
# service.yaml
apiVersion: serving.knative.dev/v1
kind: Service
metadata:
  name: hanzo-node
  annotations:
    run.googleapis.com/ingress: all
spec:
  template:
    metadata:
      annotations:
        autoscaling.knative.dev/minScale: "1"
        autoscaling.knative.dev/maxScale: "100"
        run.googleapis.com/cpu-throttling: "false"
    spec:
      containers:
      - image: gcr.io/PROJECT_ID/hanzo-node:latest
        ports:
        - containerPort: 3690
        env:
        - name: NODE_API_PORT
          value: "3690"
        resources:
          limits:
            cpu: "4"
            memory: "16Gi"
        startupProbe:
          httpGet:
            path: /v2/health
          initialDelaySeconds: 10
          periodSeconds: 10
          failureThreshold: 3
```

### Azure Container Instances

```json
{
  "type": "Microsoft.ContainerInstance/containerGroups",
  "apiVersion": "2021-10-01",
  "name": "hanzo-node",
  "location": "westus2",
  "properties": {
    "containers": [
      {
        "name": "hanzo-node",
        "properties": {
          "image": "hanzoai/hanzo-node:latest",
          "ports": [
            {"port": 3690, "protocol": "TCP"},
            {"port": 3691, "protocol": "TCP"},
            {"port": 3692, "protocol": "TCP"}
          ],
          "environmentVariables": [
            {"name": "NODE_API_PORT", "value": "3690"},
            {"name": "OPENAI_API_KEY", "secureValue": "***"}
          ],
          "resources": {
            "requests": {
              "memoryInGB": 16,
              "cpu": 4
            }
          },
          "volumeMounts": [
            {
              "name": "data",
              "mountPath": "/data"
            }
          ]
        }
      }
    ],
    "osType": "Linux",
    "ipAddress": {
      "type": "Public",
      "ports": [
        {"port": 3690, "protocol": "TCP"},
        {"port": 3692, "protocol": "TCP"}
      ],
      "dnsNameLabel": "hanzo-node"
    },
    "volumes": [
      {
        "name": "data",
        "azureFile": {
          "shareName": "hanzo-data",
          "storageAccountName": "hanzostorage",
          "storageAccountKey": "***"
        }
      }
    ]
  }
}
```

## Configuration Management

### Environment Variables

```bash
# /opt/hanzo/config/hanzo.env

# Core Configuration
NODE_NAME=hanzo-prod-01
NODE_IP=0.0.0.0
NODE_API_PORT=3690
NODE_PORT=3691
NODE_WS_PORT=3692

# Database
DATABASE_URL=/data/db.sqlite
DATABASE_MAX_CONNECTIONS=100
DATABASE_CONNECTION_TIMEOUT=30

# LanceDB
LANCEDB_PATH=/data/lancedb
LANCEDB_INDEX_TYPE=IVF_PQ
LANCEDB_METRIC=cosine

# Logging
RUST_LOG=info,hanzo_node=debug
LOG_FORMAT=json
LOG_OUTPUT=stdout

# Performance
MAX_CONCURRENT_JOBS=50
JOB_QUEUE_SIZE=1000
TOOL_EXECUTION_TIMEOUT=300
HTTP_REQUEST_TIMEOUT=60

# Security
ENABLE_TLS=true
TLS_CERT_PATH=/certs/server.crt
TLS_KEY_PATH=/certs/server.key
ENABLE_AUTH=true
AUTH_TYPE=signature

# LLM Providers
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...
TOGETHER_API_KEY=...
# Add more as needed

# Embeddings
USE_NATIVE_EMBEDDINGS=true
EMBEDDINGS_SERVER_URL=http://localhost:11434
EMBEDDING_MODEL=qwen3-2b
EMBEDDING_CACHE_SIZE=10000

# Monitoring
ENABLE_METRICS=true
METRICS_PORT=9090
ENABLE_TRACING=true
TRACING_ENDPOINT=http://jaeger:4317
```

### Production Config File

```yaml
# /opt/hanzo/config/production.yaml
node:
  name: hanzo-prod-01
  cluster_id: prod-cluster
  region: us-west-2
  
api:
  host: 0.0.0.0
  port: 3690
  max_body_size: 100MB
  rate_limit:
    requests_per_minute: 1000
    burst_size: 100
  cors:
    enabled: true
    origins:
      - "https://app.hanzo.ai"
      - "https://admin.hanzo.ai"
    
database:
  path: /data/db.sqlite
  connection_pool:
    max_connections: 100
    min_connections: 10
    connection_timeout: 30s
    idle_timeout: 600s
    max_lifetime: 1800s
  backup:
    enabled: true
    schedule: "0 2 * * *"  # 2 AM daily
    retention_days: 30
    
storage:
  lancedb:
    path: /data/lancedb
    index:
      type: IVF_PQ
      nprobe: 20
      nlist: 1000
    compaction:
      enabled: true
      schedule: "0 3 * * 0"  # 3 AM Sunday
      
security:
  tls:
    enabled: true
    cert_path: /certs/server.crt
    key_path: /certs/server.key
    client_ca_path: /certs/ca.crt
  authentication:
    enabled: true
    type: signature
    session_timeout: 3600
  encryption:
    data_at_rest: true
    key_rotation_days: 90
    
monitoring:
  prometheus:
    enabled: true
    port: 9090
    path: /metrics
  logging:
    level: info
    format: json
    outputs:
      - type: file
        path: /logs/hanzo.log
        max_size: 100MB
        max_files: 10
      - type: stdout
  tracing:
    enabled: true
    endpoint: http://jaeger:4317
    sample_rate: 0.1
```

## Monitoring Setup

### 1. Prometheus Configuration

```yaml
# prometheus.yml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'hanzo-node'
    static_configs:
      - targets: ['hanzo-node:9090']
    metrics_path: /metrics
    
  - job_name: 'node-exporter'
    static_configs:
      - targets: ['node-exporter:9100']
      
  - job_name: 'cadvisor'
    static_configs:
      - targets: ['cadvisor:8080']

alerting:
  alertmanagers:
    - static_configs:
        - targets: ['alertmanager:9093']

rule_files:
  - 'alerts.yml'
```

### 2. Alert Rules

```yaml
# alerts.yml
groups:
  - name: hanzo_alerts
    interval: 30s
    rules:
      - alert: HighMemoryUsage
        expr: (node_memory_MemTotal_bytes - node_memory_MemAvailable_bytes) / node_memory_MemTotal_bytes > 0.9
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High memory usage detected"
          description: "Memory usage is above 90% (current value: {{ $value }})"
          
      - alert: HighCPUUsage
        expr: rate(process_cpu_seconds_total[5m]) > 0.9
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High CPU usage detected"
          
      - alert: JobQueueBacklog
        expr: hanzo_job_queue_pending > 100
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: "Job queue backlog detected"
          
      - alert: APIHighLatency
        expr: histogram_quantile(0.95, rate(http_request_duration_seconds_bucket[5m])) > 5
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "API latency is high"
          
      - alert: DiskSpaceWarning
        expr: node_filesystem_avail_bytes{mountpoint="/"} / node_filesystem_size_bytes{mountpoint="/"} < 0.1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Low disk space"
```

### 3. Grafana Dashboard

```json
{
  "dashboard": {
    "title": "Hanzo Node Monitoring",
    "panels": [
      {
        "title": "API Request Rate",
        "targets": [
          {
            "expr": "rate(http_requests_total[5m])",
            "legendFormat": "{{method}} {{path}}"
          }
        ]
      },
      {
        "title": "Job Processing",
        "targets": [
          {
            "expr": "hanzo_jobs_processing",
            "legendFormat": "Processing"
          },
          {
            "expr": "hanzo_jobs_pending",
            "legendFormat": "Pending"
          }
        ]
      },
      {
        "title": "LLM Provider Usage",
        "targets": [
          {
            "expr": "rate(llm_requests_total[5m])",
            "legendFormat": "{{provider}} {{model}}"
          }
        ]
      },
      {
        "title": "Tool Executions",
        "targets": [
          {
            "expr": "rate(tool_executions_total[5m])",
            "legendFormat": "{{tool}} {{runtime}}"
          }
        ]
      }
    ]
  }
}
```

## Security Hardening

### 1. Network Security

```bash
# UFW firewall rules
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw allow 22/tcp  # SSH
sudo ufw allow 443/tcp # HTTPS
sudo ufw allow 3690/tcp # API
sudo ufw allow 3691/tcp # P2P
sudo ufw allow 3692/tcp # WebSocket
sudo ufw enable
```

### 2. TLS Configuration

```bash
# Generate certificates with Let's Encrypt
sudo certbot certonly --standalone -d api.hanzo.ai

# Strong TLS configuration for nginx
ssl_protocols TLSv1.2 TLSv1.3;
ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES128-GCM-SHA256;
ssl_prefer_server_ciphers off;
ssl_session_timeout 1d;
ssl_session_cache shared:SSL:10m;
ssl_stapling on;
ssl_stapling_verify on;
```

### 3. Secrets Management

```bash
# Use HashiCorp Vault
vault kv put secret/hanzo/api-keys \
    openai_key=sk-... \
    anthropic_key=sk-ant-...

# Or use cloud provider secrets
aws secretsmanager create-secret \
    --name hanzo/production \
    --secret-string file://secrets.json
```

### 4. Security Scanning

```bash
# Container scanning
trivy image hanzoai/hanzo-node:latest

# Dependency scanning
cargo audit

# SAST scanning
semgrep --config=auto .
```

## Backup & Recovery

### 1. Automated Backups

```bash
#!/bin/bash
# /opt/hanzo/scripts/backup.sh

BACKUP_DIR="/backups/hanzo"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_NAME="hanzo_backup_${TIMESTAMP}"

# Stop writes to database
sqlite3 /data/db.sqlite "BEGIN IMMEDIATE"

# Create backup
tar -czf ${BACKUP_DIR}/${BACKUP_NAME}.tar.gz \
    /data/db.sqlite \
    /data/lancedb \
    /opt/hanzo/config

# Resume writes
sqlite3 /data/db.sqlite "COMMIT"

# Upload to S3
aws s3 cp ${BACKUP_DIR}/${BACKUP_NAME}.tar.gz \
    s3://hanzo-backups/${BACKUP_NAME}.tar.gz

# Clean old backups (keep 30 days)
find ${BACKUP_DIR} -name "*.tar.gz" -mtime +30 -delete
```

### 2. Recovery Procedure

```bash
#!/bin/bash
# /opt/hanzo/scripts/restore.sh

BACKUP_FILE=$1

if [ -z "$BACKUP_FILE" ]; then
    echo "Usage: $0 <backup_file>"
    exit 1
fi

# Stop service
sudo systemctl stop hanzo-node

# Backup current data
mv /data /data.old

# Extract backup
tar -xzf ${BACKUP_FILE} -C /

# Verify database integrity
sqlite3 /data/db.sqlite "PRAGMA integrity_check"

# Start service
sudo systemctl start hanzo-node

# Verify service health
curl http://localhost:3690/v2/health
```

## Scaling Strategies

### Horizontal Scaling

```yaml
# HAProxy configuration for load balancing
global
    maxconn 4096
    log /dev/log local0
    
defaults
    mode http
    timeout connect 5000ms
    timeout client 50000ms
    timeout server 50000ms
    
frontend hanzo_frontend
    bind *:80
    bind *:443 ssl crt /etc/ssl/certs/hanzo.pem
    redirect scheme https if !{ ssl_fc }
    
    default_backend hanzo_backend
    
backend hanzo_backend
    balance leastconn
    option httpchk GET /v2/health
    
    server node1 10.0.1.10:3690 check
    server node2 10.0.1.11:3690 check
    server node3 10.0.1.12:3690 check
```

### Vertical Scaling

```bash
# Adjust system limits
echo "* soft nofile 65536" >> /etc/security/limits.conf
echo "* hard nofile 65536" >> /etc/security/limits.conf

# Tune kernel parameters
cat >> /etc/sysctl.conf << EOF
net.core.rmem_max = 134217728
net.core.wmem_max = 134217728
net.ipv4.tcp_rmem = 4096 87380 134217728
net.ipv4.tcp_wmem = 4096 65536 134217728
net.core.netdev_max_backlog = 5000
EOF

sudo sysctl -p
```

## Troubleshooting

### Common Issues

#### High Memory Usage
```bash
# Check memory usage
free -h
ps aux --sort=-%mem | head

# Clear caches
echo 3 > /proc/sys/vm/drop_caches

# Restart with memory limit
systemd-run --uid=hanzo --gid=hanzo \
    --setenv=RUST_LOG=info \
    -p MemoryLimit=8G \
    /opt/hanzo/bin/hanzod
```

#### Database Lock Issues
```bash
# Check for locks
lsof /data/db.sqlite

# Force unlock (use with caution)
rm /data/db.sqlite-shm
rm /data/db.sqlite-wal

# Vacuum database
sqlite3 /data/db.sqlite "VACUUM;"
```

#### Connection Issues
```bash
# Check listening ports
ss -tulpn | grep hanzo

# Test connectivity
curl -v http://localhost:3690/v2/health

# Check firewall rules
sudo iptables -L -n -v
```

#### Performance Issues
```bash
# Profile CPU usage
perf top -p $(pgrep hanzod)

# Trace system calls
strace -p $(pgrep hanzod) -c

# Check I/O stats
iotop -p $(pgrep hanzod)
```

### Debug Mode

```bash
# Enable debug logging
export RUST_LOG=debug,hanzo_node=trace
export RUST_BACKTRACE=full

# Run with verbose output
/opt/hanzo/bin/hanzod --verbose

# Enable core dumps
ulimit -c unlimited
echo "/tmp/core-%e-%p-%t" > /proc/sys/kernel/core_pattern
```

### Health Check Script

```bash
#!/bin/bash
# /opt/hanzo/scripts/health_check.sh

API_URL="http://localhost:3690/v2/health"
MAX_RETRIES=3
RETRY_DELAY=5

for i in $(seq 1 $MAX_RETRIES); do
    response=$(curl -s -o /dev/null -w "%{http_code}" $API_URL)
    
    if [ "$response" == "200" ]; then
        echo "Health check passed"
        exit 0
    fi
    
    echo "Health check failed (attempt $i/$MAX_RETRIES)"
    sleep $RETRY_DELAY
done

echo "Health check failed after $MAX_RETRIES attempts"
exit 1
```

## Production Checklist

### Pre-Deployment
- [ ] Hardware requirements met
- [ ] OS and dependencies updated
- [ ] TLS certificates obtained
- [ ] Firewall rules configured
- [ ] Backup strategy defined
- [ ] Monitoring setup completed
- [ ] Load testing performed
- [ ] Security scan passed
- [ ] Documentation reviewed

### Deployment
- [ ] Environment variables configured
- [ ] Secrets securely stored
- [ ] Database initialized
- [ ] Services started and healthy
- [ ] Logs being collected
- [ ] Metrics being scraped
- [ ] Alerts configured
- [ ] DNS configured
- [ ] Load balancer configured

### Post-Deployment
- [ ] Health checks passing
- [ ] Performance baseline established
- [ ] Backup job scheduled
- [ ] Team access configured
- [ ] Incident response plan ready
- [ ] Runbook documented
- [ ] Customer notification sent

---

*For additional support, contact the Hanzo DevOps team or consult the [Operations Manual](https://docs.hanzo.ai/operations).*