# Airstack

**Modular, type-safe infrastructure SDK and CLI for lightweight provisioning and deployment workflows.**

Airstack combines the power of Rust for execution with TypeScript for developer experience, using TOML for declarative configuration. Built for simplicity and extensibility.

## Features

- 🦀 **Rust Core**: Fast, reliable execution engine
- 📦 **Zero Vendor Lock-in**: Provider-agnostic abstractions
- 🔧 **Type-Safe**: Full TypeScript support with validation
- 🏗️ **Declarative Config**: Simple TOML configuration
- 🚀 **Easy Installation**: Single npm install
- 🔌 **Extensible**: Plugin architecture for providers

## Quick Start

### Installation

```bash
npm install -g airstack
```

### Initialize a Project

```bash
mkdir my-infra && cd my-infra
airstack init
```

### Configure Your Infrastructure

Edit the generated `airstack.toml`:

```toml
[project]
name = "my-project"
description = "My awesome infrastructure"

[[infra.servers]]
name = "web-server"
provider = "hetzner"
region = "nbg1"
server_type = "cx21"
ssh_key = "~/.ssh/id_ed25519.pub"
floating_ip = true

[services.nginx]
image = "nginx:latest"
ports = [80, 443]
env = { ENVIRONMENT = "production" }

[services.app]
image = "myapp:latest"
ports = [3000]
depends_on = ["nginx"]
```

### Deploy

```bash
# Set your Hetzner API token
export HETZNER_API_KEY="your-token-here"
# (also supported: HETZNER_API_TOKEN, HETZNER_TOKEN)

# Provision infrastructure
airstack up

# Deploy services
airstack deploy nginx
airstack deploy app
airstack deploy all

# Scale a service
airstack scale app 3

# Check status
airstack status
```

AirStack also loads a global env file (first match):
- `$AIRSTACK_ENV_FILE`
- `$AIRSTACK_HOME/.env`
- `~/.airstack/.env`
- `~/.config/airstack/.env`

This lets you keep provider keys in one AirStack-local place instead of per-project `.env` files.

## Commands

| Command | Description |
|---------|-------------|
| `airstack init [name] [--provider hetzner|fly] [--preset clickhouse]` | Initialize a project with provider/service presets |
| `airstack up [--local] [--bootstrap-runtime] [--auto-fallback] [--resolve-capacity]` | Provision infrastructure (or explicit local mode) with optional runtime bootstrap |
| `airstack destroy` | Destroy infrastructure |
| `airstack deploy &lt;service&gt; [--latest-code --push --tag <tag>] [--strategy rolling\|bluegreen\|canary]` | Deploy a service (optional latest-code + strategy mode) |
| `airstack cexec &lt;server&gt; &lt;container&gt; [cmd...]` | Execute a command inside a remote container |
| `airstack scale &lt;service&gt; &lt;replicas&gt;` | Scale service replicas |
| `airstack cli` | Launch lightweight interactive menu CLI |
| `airstack tui [--view <name>]` | Launch FrankenTUI interface |
| `airstack script <list|plan|run>` | Run remote lifecycle scripts defined in config |
| `airstack status [--source auto|provider|ssh|control-plane]` | Show status with source-of-truth mode |
| `airstack ssh &lt;server&gt;` | SSH into a server |
| `airstack logs &lt;service&gt;` | Show service logs |
| `airstack plan [--auto-fallback] [--resolve-capacity]` | Preview create/update/destroy and deploy actions with infra compatibility preflight |
| `airstack apply` | Apply desired infrastructure and services |
| `airstack edge &lt;plan|apply|validate|status&gt;` | Reverse-proxy workflows |
| `airstack edge diagnose` | TLS/ACME diagnosis with remediation hints |
| `airstack doctor` | Validate production safety and policy checks |
| `airstack drift` | Detect config image tag vs running image drift |
| `airstack registry doctor [--server <name>] --image <image>` | Verify remote registry pull credentials/scope |
| `airstack reconcile [--dry-run] [--detailed]` | Idempotent converge-to-config workflow |
| `airstack go-live` | One-shot go-live readiness (infra + image pull + edge DNS/TLS + app health) |
| `airstack runbook` | Print operational command runbook |
| `airstack secrets &lt;set|get|list|delete&gt;` | Encrypted local secrets management |
| `airstack backup &lt;enable|status|restore&gt;` | Managed backup lifecycle |
| `airstack release &lt;service&gt; [--push] [--update-config]` | Build/publish release images |
| `airstack ship &lt;service&gt; [--push --update-config] [--strategy rolling\|bluegreen\|canary]` | Atomic release+deploy with rollback on deploy failure |

### Output Modes

- `--json`: machine-readable structured output
- `--quiet`: suppress human-readable output
- `--env <name>`: load environment overlay from `airstack.<name>.toml`
- `--allow-local-deploy`: bypass remote-first deploy guard when infra exists
- `up --local`: explicit local verification mode (skips infra provisioning)
- `up --bootstrap-runtime`: install Docker on remote hosts before service deploy

### Fozzy Gate

Run the production Fozzy suite (deterministic checks + host-backed CLI quality gate):

```bash
./scripts/fozzy-suite.sh
```

### TUI Runtime (FrankenTUI)

Airstack integrates [FrankenTUI](https://github.com/Dicklesworthstone/frankentui) as an optional Rust feature.

Default builds exclude TUI to keep compile times and binary footprint lower:

```bash
cargo build -p airstack-core
```

Enable TUI explicitly when needed:

```bash
cargo build -p airstack-core --features tui
cargo run -p airstack-core --features tui -- tui
```

Optional view targeting:

```bash
airstack tui --view dashboard
airstack tui --view services
airstack tui --view logs
airstack tui --view ssh
```

TUI shortcuts:
- `:` open command palette
- `Tab` cycle focus panes
- `j/k` or arrow keys switch views
- `1..9` jump directly to a view
- `q` or `Esc` quit

TUI views:
- Dashboard
- Servers
- Services
- Logs
- Scaling
- Network
- Providers
- SSH
- Settings

## Configuration

### Infrastructure Providers

Currently supported:

- **Hetzner Cloud** (`hetzner`)
  - Set `HETZNER_API_KEY` (or `HETZNER_API_TOKEN` / `HETZNER_TOKEN`) environment variable
  - Supports all server types and regions
  - Region policy: if `region` is omitted, default is `ash`; `region="auto"` or `--resolve-capacity` picks a valid region for the requested server type
- **Fly.io Machines** (`fly`)
  - Uses `flyctl` for provider operations
  - Auth resolution order: provider token -> `FLY_API_TOKEN` -> `FLY_ACCESS_TOKEN` -> local `flyctl auth`
  - Supports app/machine inventory, machine create/destroy, provider-native SSH (`flyctl ssh console`), and Fly-native workload inventory in `airstack status`
  - `airstack cexec <fly-server> <container> -- <cmd...>` maps to `flyctl ssh console --container ...`

### Container Runtimes

Currently supported:

- **Docker** (`docker`)
  - Requires Docker daemon running
  - Supports all Docker features

### Example Configuration

```toml
[project]
name = "production-app"
description = "Production deployment"
deploy_mode = "remote"

# Multiple servers
[[infra.servers]]
name = "web-1"
provider = "hetzner"
region = "nbg1"
server_type = "cx21"
ssh_key = "~/.ssh/id_ed25519.pub"

[[infra.servers]]
name = "web-2"
provider = "hetzner"
region = "fsn1"
server_type = "cx21"
ssh_key = "~/.ssh/id_ed25519.pub"

[infra.firewall]
name = "web-ingress"
ingress = [
  { protocol = "tcp", port = "22", source_ips = ["203.0.113.0/24"] },
  { protocol = "tcp", port = "80", source_ips = ["0.0.0.0/0", "::/0"] },
  { protocol = "tcp", port = "443", source_ips = ["0.0.0.0/0", "::/0"] }
]

[[infra.servers]]
name = "edge-fly"
provider = "fly"
region = "iad"
server_type = "shared-cpu-1x"
ssh_key = "~/.ssh/id_ed25519.pub"

# Services with dependencies
[services.database]
image = "postgres:15"
ports = [5432]
env = { POSTGRES_DB = "myapp", POSTGRES_PASSWORD = "secret" }
volumes = ["./data:/var/lib/postgresql/data"]

[services.api]
image = "myapp/api:v1.2.0"
ports = [3000]
depends_on = ["database"]
env = { DATABASE_URL = "postgres://postgres:secret@database:5432/myapp" }

[services.frontend]
image = "myapp/frontend:v1.2.0"
ports = [80, 443]
depends_on = ["api"]
env = { API_URL = "http://api:3000" }

[edge]
provider = "caddy"

[[edge.sites]]
host = "api.example.com"
upstream_service = "frontend"
upstream_port = 80
tls_email = "ops@example.com"
redirect_http = true

[scripts.bootstrap]
target = "all"
file = "scripts/bootstrap.sh"
idempotency = "once"
timeout_secs = 300

[scripts.migrate]
target = "server:web-1"
file = "scripts/migrate.sh"
idempotency = "on-change"
retry = { max_attempts = 2, transient_only = true }

[hooks]
pre_provision = ["bootstrap"]
post_deploy = ["migrate"]
```

Remote deploy note: bind-mount sources for remote services must be absolute paths on the remote host (for example `/opt/airstack/data:/var/lib/postgresql/data`). Relative/local paths are rejected during deploy preflight.

## Development

### Prerequisites

- Rust 1.70+
- Node.js 18+
- Docker (for container features)
- `flyctl` (if using `provider = "fly"`)

### Build from Source

```bash
git clone https://github.com/saint0x/airstack
cd airstack
make install
```

### Development Commands

```bash
make build          # Build debug version
make build-release  # Build release version
make test           # Run tests
make dev            # Development mode with file watching
make lint           # Lint code
make fmt            # Format code
```

## Architecture

```
┌─────────────────┐    ┌──────────────────┐
│  TypeScript CLI │────│   Rust Binary    │
│   (npm package) │    │  (cross-platform)│
└─────────────────┘    └──────────────────┘
                              │
         ┌────────────────────┼────────────────────┐
         │                    │                    │
    ┌─────────┐         ┌──────────┐        ┌──────────┐
    │ Config  │         │  Metal   │        │Container │
    │ (TOML)  │         │Providers │        │Providers │
    └─────────┘         └──────────┘        └──────────┘
                              │                    │
                        ┌──────────┐        ┌──────────┐
                        │ Hetzner  │        │  Docker  │
                        │   API    │        │   API    │
                        └──────────┘        └──────────┘
```

### Core Components

- **Config Loader**: TOML parsing and validation
- **Metal Providers**: Bare metal server provisioning
- **Container Providers**: Container orchestration
- **CLI Core**: Command routing and execution
- **TypeScript Wrapper + SDK**: npm distribution, typed config helpers, and binary-backed client API

## TypeScript SDK

```ts
import { AirstackClient } from 'airstack';

const client = new AirstackClient({ configPath: './airstack.toml' });
const status = await client.statusJson(true);
console.log(status);
```

## Extending Airstack

### Adding a Provider

1. Create a new crate in `crates/`
2. Implement the provider trait
3. Register in the provider factory
4. Add configuration schema

Example:

```rust
// crates/metal/src/digitalocean.rs
#[async_trait::async_trait]
impl MetalProvider for DigitalOceanProvider {
    async fn create_server(&self, request: CreateServerRequest) -> Result<Server> {
        // Implementation
    }
    // ... other methods
}
```

### Provider Plugin System

Future versions will support external provider plugins:

```toml
[providers]
aws = { plugin = "airstack-aws", version = "1.0" }
gcp = { plugin = "airstack-gcp", version = "1.0" }
```

## Roadmap

- [ ] AWS Provider
- [ ] Google Cloud Provider
- [ ] Kubernetes Support
- [ ] Terraform Integration
- [ ] GitOps Workflows
- [ ] Monitoring & Alerting
- [ ] Zero-downtime Deployments

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests
5. Submit a pull request

## License

MIT License - see [LICENSE](LICENSE) for details.

## Support

- 📚 [Documentation](https://docs.airstack.dev)
- 🐛 [Issues](https://github.com/airstack/airstack/issues)
- 💬 [Discussions](https://github.com/airstack/airstack/discussions)
- 🔧 [Examples](https://github.com/airstack/examples)
