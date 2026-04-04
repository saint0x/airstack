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

# Check status (works from project root or any subdirectory)
airstack status

# Aggregate status for all discovered stacks from anywhere
# (uses AIRSTACK_REPO + Desktop discovery when no local config is found)
AIRSTACK_REPO=~/Desktop airstack status
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
| `airstack up [--local] [--bootstrap-runtime] [--auto-fallback] [--resolve-capacity] [--ensure-host-paths]` | Provision infrastructure (or explicit local mode) with optional runtime bootstrap |
| `airstack destroy` | Destroy infrastructure |
| `airstack deploy &lt;service&gt; [--latest-code --push] [--tag <tag>] [--strategy rolling\|bluegreen\|canary] [--ensure-host-paths]` | Deploy a service (`--latest-code` auto-falls back to remote build in remote deploy mode when local Docker is unavailable) |
| `airstack cexec &lt;server&gt; &lt;container&gt; [--cmd "<shell>"] [--script <path>] [-- <argv...>]` | Execute inside a remote container (shell, script, or raw argv mode) |
| `airstack scale &lt;service&gt; &lt;replicas&gt;` | Scale service replicas |
| `airstack cli` | Launch lightweight interactive menu CLI |
| `airstack tui [--view <name>]` | Launch FrankenTUI interface |
| `airstack script <list|plan|run>` | Run remote lifecycle scripts defined in config |
| `airstack status [--source auto|provider|ssh|control-plane]` | Show status with source-of-truth mode (includes deploy provenance fields in JSON). If no local config is resolved, auto-discovers and aggregates all stack statuses. |
| `airstack ssh &lt;server&gt; [--cmd "<shell>"] [--script <path>] [-- <argv...>]` | SSH into a server (shell, script, or raw argv mode) |
| `airstack logs &lt;service&gt;` | Show service logs |
| `airstack plan [--auto-fallback] [--resolve-capacity]` | Preview create/update/destroy and deploy actions with infra compatibility preflight |
| `airstack apply [--ensure-host-paths]` | Apply desired infrastructure and services |
| `airstack edge &lt;plan|apply|validate|status&gt;` | Reverse-proxy workflows |
| `airstack edge diagnose` | TLS/ACME diagnosis with remediation hints |
| `airstack upload &lt;server&gt; &lt;src&gt; &lt;dest&gt; [--checksum <sha256>]` | Upload artifact with checksum verification and atomic move |
| `airstack cp &lt;server&gt; &lt;src&gt; &lt;dest&gt; [--checksum <sha256>]` | Alias for `airstack upload` |
| `airstack doctor` | Validate production safety and policy checks |
| `airstack drift` | Detect config image tag vs running image drift |
| `airstack registry doctor [--server <name>] --image <image>` | Verify remote registry pull credentials/scope |
| `airstack reconcile [--dry-run] [--detailed]` | Idempotent converge-to-config workflow |
| `airstack go-live` | One-shot go-live readiness (infra + image pull + edge DNS/TLS + app health) |
| `airstack runbook` | Print operational command runbook |
| `airstack secrets &lt;set|get|list|delete&gt;` | Encrypted local secrets management |
| `airstack backup &lt;enable|status|restore&gt;` | Managed backup lifecycle |
| `airstack provider profile <list|show|set|use|remove|snapshot|status>` | First-class provider profile management (Fly and any provider/custom env context) |
| `airstack provider inventory <provider> [--profile <name> ... | --all-profiles]` | Provider resource inventory by profile (for Fly: apps/machines/account metadata) |
| `airstack release &lt;service&gt; [--push] [--update-config] [--remote-build <server>] [--from build\|push]` | Build/publish release images with structured phase output and phase resume |
| `airstack ship &lt;service&gt; [--push --update-config] [--strategy rolling\|bluegreen\|canary]` | Atomic release+deploy with rollback on deploy failure |

### Output Modes

- `--json`: machine-readable structured output
- `--quiet`: suppress human-readable output
- `--env <name>`: load environment overlay from `airstack.<name>.toml`
- `--allow-local-deploy`: bypass remote-first deploy guard when infra exists
- `up --local`: explicit local verification mode (skips infra provisioning)
- `up --bootstrap-runtime`: install Docker on remote hosts before service deploy
- `--ensure-host-paths`: auto-create missing remote bind-mount host paths during deploy preflight
- `--provider-profile <provider>:<profile>`: override provider profile for current command

### Provider Profiles

Profiles are persisted in `~/.airstack/provider_profiles.json` and can inject provider-specific env vars per run.
Optional repo-level pinning is supported:

```toml
[providers.profiles]
fly = "work"
```

Mutating commands (`up`, `deploy`, `destroy`, `apply`, `ship`) print provider profile/account preflight context and require confirmation unless `-y` is passed.

```bash
# Snapshot current Fly config as work profile and activate it
airstack provider profile snapshot fly work \
  --source ~/.fly \
  --config-env FLYCTL_CONFIG_DIR \
  --activate

# After re-authenticating Fly to personal account, snapshot again
airstack provider profile snapshot fly personal \
  --source ~/.fly \
  --config-env FLYCTL_CONFIG_DIR

# Switch active Fly profile
airstack provider profile use fly personal

# Run one command against a specific profile without changing active profile
airstack --provider-profile fly:work status --source provider

# Compare status across all Fly profiles
airstack provider profile status fly --detailed

# Scriptable status payload across profiles
airstack provider profile status fly --json

# Provider-native inventory (decoupled from current project targets)
airstack provider inventory fly --all-profiles --json
```

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
  - `airstack cexec <fly-server> <container> -- <cmd...>` and `--cmd "<shell>"` map to `flyctl ssh console --container ...`

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
extra_include_file = "/opt/aria/Caddy.extra"

[[edge.sites]]
host = "api.example.com"
upstream_service = "frontend"
upstream_port = 80
tls_email = "ops@example.com"
redirect_http = true

[[edge.sites.static]]
path_prefix = "/downloads"
root = "/srv/downloads"
browse = false
host_path = "/srv/downloads"
headers = { Cache-Control = "public, max-age=300" }

[[edge.sites.routes]]
path_prefix = "/internal"
upstream = "frontend:8081"
strip_prefix = true
headers = { X-Edge-Route = "internal" }

# Single production method for nearest-region routing:
# geo-header steering (for example CF-IPCountry from a trusted edge)
[edge.sites.nearest]
method = "geo-header"
header = "CF-IPCountry"
default_upstream = "api_us:8080"

[[edge.sites.nearest.regions]]
name = "eu"
countries = ["GB", "IE", "DE", "FR", "NL", "ES", "IT", "SE", "NO", "DK", "FI", "PL"]
upstream = "api_eu:8080"

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
If host paths are missing, Airstack now prints a ready-to-run remediation command and you can retry with `--ensure-host-paths` to auto-create them.

## Common Recipes

### Static DMG downloads behind Caddy

```toml
[edge]
provider = "caddy"
extra_include_file = "/opt/aria/Caddy.extra"

[[edge.sites]]
host = "downloads.example.com"
upstream_service = "frontend"
upstream_port = 80
tls_email = "ops@example.com"

[[edge.sites.static]]
path_prefix = "/downloads"
root = "/srv/downloads"
browse = false
host_path = "/srv/downloads"
headers = { Cache-Control = "public, max-age=300" }
```

```bash
airstack upload edge-1 ./dist/MyApp-1.2.3.dmg /srv/downloads/MyApp-1.2.3.dmg
airstack upload edge-1 ./dist/MyApp-1.2.3.dmg.sha256 /srv/downloads/MyApp-1.2.3.dmg.sha256
airstack ssh edge-1 --cmd "ln -sfn /srv/downloads/MyApp-1.2.3.dmg /srv/downloads/MyApp-latest.dmg"
airstack edge apply
```

### Stable custom Caddy rules that survive edge apply

Put custom directives in the configured include file (for example `/opt/aria/Caddy.extra`).
Airstack writes the generated Caddyfile and imports this file without overwriting it.

### Inspect edge source-of-truth and drift

```bash
airstack edge status
```

`edge status` now reports:
- managed Caddyfile path(s)
- generated-file overwrite warning
- include file path (when configured)
- rendered-vs-live config diff preview

### Nearest-region routing (single supported method)

Airstack supports one native nearest-routing method for Caddy edge: `geo-header`.
Use `[edge.sites.nearest]` with `method = "geo-header"` and provide region country maps.

- `header`: trusted country header name (default: `CF-IPCountry`)
- `regions[].countries`: ISO-3166 alpha-2 country codes
- `regions[].upstream`: upstream target (`service:port`)
- `default_upstream`: fallback upstream when no region matches

All requests for the site are steered through this map after explicit static/path routes.
This keeps one deterministic routing surface for all backend endpoints.

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
