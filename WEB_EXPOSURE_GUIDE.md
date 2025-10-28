# Web Exposure Guide for Hanzo Node

This guide shows how to expose your Hanzo Node to the public internet using various tunneling services. This enables:
- Public API access for your embeddings/reranking/LLM services
- Remote access to your Hanzo instance
- Integration with external applications
- Sharing your models with others

## Prerequisites

Ensure your Hanzo Node is running:
```bash
# Start hanzod with web exposure enabled
hanzod --web-enabled --api-port 3690 --p2p-port 3691 --web-port 3692

# Or using environment variables
export WEB_ENABLED=true
export API_PORT=3690  # Main hanzod port (API + WebSocket)
export P2P_PORT=3691  # P2P consensus port
export WEB_PORT=3692  # Web interface port
sh scripts/run_node_localhost.sh
```

**Port Configuration (sequential from 3690):**
- **3690**: Main hanzod port (REST API + WebSocket on same port)
- **3691**: P2P consensus port for node-to-node communication
- **3692**: Web interface (if separate UI is enabled)

## Option 1: Ngrok (Easiest Setup)

### Installation
```bash
# macOS
brew install ngrok

# Linux
curl -s https://ngrok-agent.s3.amazonaws.com/ngrok.asc | sudo tee /etc/apt/trusted.gpg.d/ngrok.asc >/dev/null
echo "deb https://ngrok-agent.s3.amazonaws.com buster main" | sudo tee /etc/apt/sources.list.d/ngrok.list
sudo apt update && sudo apt install ngrok

# Sign up for free account at https://ngrok.com
ngrok config add-authtoken YOUR_AUTH_TOKEN
```

### Expose Hanzo API
```bash
# Expose main hanzod port (3690) - includes API + WebSocket
ngrok http 3690 --domain your-hanzo.ngrok.io

# Expose both API and Web interface
ngrok start --all --config ~/.hanzo/config/ngrok.yml
```

### Ngrok Configuration (~/.hanzo/config/ngrok.yml)
```yaml
version: 2
authtoken: YOUR_AUTH_TOKEN
tunnels:
  hanzo-api:
    addr: 3690
    proto: http
    hostname: api.your-hanzo.ngrok.io
    inspect: false
    bind_tls: true
    # WebSocket support is automatic with ngrok
  hanzo-web:
    addr: 3692
    proto: http
    hostname: web.your-hanzo.ngrok.io
    bind_tls: true
```

### Test Your Endpoints
```bash
# Test REST API endpoint
curl https://api.your-hanzo.ngrok.io/v2/embeddings \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-embedding-8b",
    "input": "Hello from the internet!"
  }'

# Test WebSocket connection (using wscat)
npm install -g wscat
wscat -c wss://api.your-hanzo.ngrok.io/ws

# Or with Python
python -c "
import websocket
ws = websocket.WebSocket()
ws.connect('wss://api.your-hanzo.ngrok.io/ws')
ws.send('ping')
print(ws.recv())
ws.close()
"
```

## Option 2: LocalXpose (Alternative to Ngrok)

### Installation
```bash
# Download LocalXpose
curl -O https://loclx.io/cli/linux/loclx.zip
unzip loclx.zip
sudo mv loclx /usr/local/bin/
chmod +x /usr/local/bin/loclx

# Login (get access token from https://localxpose.io)
loclx account login
```

### Expose Hanzo Services
```bash
# Expose main hanzod API/WebSocket
loclx tunnel http --to 3690 --subdomain hanzo-api

# Expose web interface
loclx tunnel http --to 3692 --subdomain hanzo-web

# Run both in background
loclx tunnel http --to 3690 --subdomain hanzo-api --bg
loclx tunnel http --to 3692 --subdomain hanzo-web --bg
```

### LocalXpose Config (~/.hanzo/config/loclx.yml)
```yaml
tunnels:
  - name: hanzo-api
    type: http
    to: localhost:3690
    subdomain: hanzo-api
    region: us
    # Supports WebSocket automatically
  - name: hanzo-web
    type: http
    to: localhost:3692
    subdomain: hanzo-web
    region: us

# Start with config
# loclx tunnel config ~/.hanzo/config/loclx.yml
```

## Option 3: Cloudflare Tunnel (Most Robust)

### Installation
```bash
# macOS
brew install cloudflare/cloudflare/cloudflared

# Linux
wget https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64.deb
sudo dpkg -i cloudflared-linux-amd64.deb

# Authenticate with Cloudflare
cloudflared tunnel login
```

### Create Tunnel
```bash
# Create a tunnel
cloudflared tunnel create hanzo-node

# Get tunnel ID (save this)
cloudflared tunnel list

# Create DNS routes (requires your domain)
cloudflared tunnel route dns hanzo-node api.hanzo.yourdomain.com
cloudflared tunnel route dns hanzo-node web.hanzo.yourdomain.com
```

### Cloudflare Config (~/.hanzo/config/cloudflared.yml)
```yaml
tunnel: YOUR_TUNNEL_ID
credentials-file: /Users/YOUR_USER/.cloudflared/YOUR_TUNNEL_ID.json

ingress:
  # Main API/WebSocket endpoint
  - hostname: api.hanzo.yourdomain.com
    service: http://localhost:3690
    originRequest:
      noTLSVerify: false
      connectTimeout: 30s
      # WebSocket support
      httpHostHeader: "api.hanzo.yourdomain.com"

  # Web interface
  - hostname: web.hanzo.yourdomain.com
    service: http://localhost:3692
    originRequest:
      noTLSVerify: false

  # Health check endpoint
  - hostname: health.hanzo.yourdomain.com
    service: http://localhost:3690/health

  # Catch-all rule
  - service: http_status:404
```

### Run Cloudflare Tunnel
```bash
# Run with config file
cloudflared tunnel --config ~/.hanzo/config/cloudflared.yml run

# Run as service (recommended)
sudo cloudflared service install
sudo systemctl start cloudflared
sudo systemctl enable cloudflared
```

## Integration with Hanzo Node

### Update Hanzo Configuration
```bash
# Edit ~/.hanzo/config/hanzo.toml
[web]
enabled = true
host = "0.0.0.0"
web_port = 3692    # Web interface
api_port = 3690    # Main hanzod port (API + WebSocket)
p2p_port = 3691    # P2P consensus port
ws_enabled = true  # Enable WebSocket support
ws_port = null     # null means use api_port for WebSocket
enable_cors = true
allowed_origins = ["*"]

# For Ngrok
public_url = "https://api.your-hanzo.ngrok.io"

# For LocalXpose
public_url = "https://hanzo-api.loclx.io"

# For Cloudflare
public_url = "https://api.hanzo.yourdomain.com"
```

### Start Hanzo with Public URL
```bash
# With environment variable
export PUBLIC_URL="https://api.your-hanzo.ngrok.io"
hanzod --web-enabled

# Or in script
PUBLIC_URL="https://api.hanzo.yourdomain.com" \
WEB_ENABLED=true \
sh scripts/run_node_localhost.sh
```

## Security Considerations

### API Authentication
```bash
# Generate API key
hanzod generate-api-key --name "public-access"

# Use in requests
curl https://api.your-hanzo.ngrok.io/v2/embeddings \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model": "qwen3-embedding-8b", "input": "test"}'
```

### Rate Limiting
```toml
# ~/.hanzo/config/hanzo.toml
[security]
enable_rate_limiting = true
requests_per_minute = 100
requests_per_hour = 1000
enable_api_keys = true
```

### Allowed Origins (CORS)
```toml
# ~/.hanzo/config/hanzo.toml
[web]
enable_cors = true
# Restrict to specific origins in production
allowed_origins = [
  "https://yourapp.com",
  "https://app.hanzo.ai"
]
```

## Monitoring Public Access

### Check Tunnel Status
```bash
# Ngrok
ngrok api tunnels list

# LocalXpose
loclx tunnel status

# Cloudflare
cloudflared tunnel info YOUR_TUNNEL_ID
```

### View Access Logs
```bash
# Hanzo logs
tail -f ~/.hanzo/logs/access.log

# Ngrok dashboard
open http://localhost:4040

# Cloudflare dashboard
open https://one.dash.cloudflare.com/
```

## Example Client Applications

### Python Client
```python
import requests

HANZO_API = "https://api.your-hanzo.ngrok.io"
API_KEY = "your-api-key"

def get_embedding(text):
    response = requests.post(
        f"{HANZO_API}/v2/embeddings",
        headers={
            "Authorization": f"Bearer {API_KEY}",
            "Content-Type": "application/json"
        },
        json={
            "model": "qwen3-embedding-8b",
            "input": text
        }
    )
    return response.json()

# Test
embedding = get_embedding("Hello from Python!")
print(f"Embedding dimension: {len(embedding['data'][0]['embedding'])}")
```

### JavaScript Client
```javascript
const HANZO_API = 'https://api.your-hanzo.ngrok.io';
const HANZO_WS = 'wss://api.your-hanzo.ngrok.io/ws';
const API_KEY = 'your-api-key';

// REST API Example
async function getEmbedding(text) {
  const response = await fetch(`${HANZO_API}/v2/embeddings`, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${API_KEY}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({
      model: 'qwen3-embedding-8b',
      input: text
    })
  });
  return response.json();
}

// WebSocket Example
function connectWebSocket() {
  const ws = new WebSocket(HANZO_WS);

  ws.onopen = () => {
    console.log('Connected to Hanzo WebSocket');
    ws.send(JSON.stringify({
      type: 'auth',
      api_key: API_KEY
    }));
  };

  ws.onmessage = (event) => {
    console.log('Message:', JSON.parse(event.data));
  };

  return ws;
}

// Test
getEmbedding('Hello from JavaScript!')
  .then(result => console.log('Embedding:', result));

const ws = connectWebSocket();
```

## Troubleshooting

### Port Already in Use
```bash
# Find process using port
lsof -i :3690  # API/WebSocket
lsof -i :3691  # P2P
lsof -i :3692  # Web interface

# Kill process
kill -9 PID
```

### Tunnel Connection Issues
```bash
# Test local service first
curl http://localhost:3690/health

# Test WebSocket locally
wscat -c ws://localhost:3690/ws

# Check firewall
sudo ufw status
sudo ufw allow 3690  # API/WebSocket
sudo ufw allow 3691  # P2P
sudo ufw allow 3692  # Web interface

# Restart tunnel service
# Ngrok
ngrok restart

# Cloudflare
sudo systemctl restart cloudflared
```

### CORS Errors
```bash
# Update hanzo.toml
[web]
enable_cors = true
allowed_origins = ["*"]  # For testing only

# Restart hanzod
pkill hanzod
hanzod --web-enabled
```

## Production Recommendations

1. **Use Cloudflare Tunnel** for production - it's the most reliable and includes DDoS protection
2. **Enable API authentication** - Never expose endpoints without authentication
3. **Set up monitoring** - Use Prometheus/Grafana to monitor usage
4. **Configure rate limiting** - Prevent abuse and control costs
5. **Use HTTPS only** - All tunnel services provide SSL/TLS
6. **Backup your tunnel configs** - Store in ~/.hanzo/config/
7. **Set up health checks** - Monitor tunnel and service availability

## Quick Start Script

Save this as `~/.hanzo/scripts/expose-web.sh`:

```bash
#!/bin/bash

# Expose Hanzo Node to the web
SERVICE=${1:-ngrok}  # ngrok, loclx, or cloudflare

case $SERVICE in
  ngrok)
    echo "Starting Ngrok tunnel..."
    ngrok start --all --config ~/.hanzo/config/ngrok.yml
    ;;
  loclx)
    echo "Starting LocalXpose tunnel..."
    loclx tunnel config ~/.hanzo/config/loclx.yml
    ;;
  cloudflare)
    echo "Starting Cloudflare tunnel..."
    cloudflared tunnel --config ~/.hanzo/config/cloudflared.yml run
    ;;
  *)
    echo "Usage: $0 [ngrok|loclx|cloudflare]"
    exit 1
    ;;
esac
```

Make it executable:
```bash
chmod +x ~/.hanzo/scripts/expose-web.sh

# Use it
~/.hanzo/scripts/expose-web.sh ngrok
```

Your Hanzo Node is now accessible from anywhere in the world!