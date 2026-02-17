Airstack v1 — Full Project Scope (TUI + SDK DevOps Runtime)

Goal:
Build a tightly-scoped, provider-agnostic DevOps runtime that works as:
	•	A powerful TUI (primary interface)
	•	A clean CLI (scriptable layer)
	•	A type-safe SDK (programmatic control)

Airstack should feel like:
	•	“htop + flyctl + docker + ssh” in one tool
	•	Local-first, fast, composable
	•	Dead simple mental model

No overreach. No platform bloat. Just clean infra control.

Implementation Audit (as of February 17, 2026)

Legend: `✅` done, `🟢` in progress/partial, `⬜` not started.

1) Infrastructure Provisioning
- ✅ Provider trait abstraction
- ✅ Hetzner provider crate
- 🟢 Idempotent provisioning logic
- ⬜ Retry + backoff logic
- ⬜ State reconciliation

2) Service Deployment (Containers)
- ✅ Container runtime abstraction
- ✅ Docker runtime implementation
- 🟢 Service lifecycle manager
- 🟢 Dependency ordering
- ⬜ Health status tracking

3) Scaling
- 🟢 Replica tracking
- ✅ Deterministic naming
- 🟢 Rolling spawn logic
- ✅ Safe scale-down logic

4) Load Balancing
- ⬜ Embedded proxy integration
- ⬜ Upstream pool manager
- ⬜ Config auto-regeneration
- ⬜ Hot reload support

5) Logs
- 🟢 Log streaming layer
- ⬜ Multiplexed log router
- ⬜ Persistent scrollback buffer
- ⬜ Structured log mode

6) SSH + Remote Control
- 🟢 SSH connection manager
- 🟢 Key resolution logic
- ⬜ TUI terminal embedding
- ⬜ Session multiplexing

7) Status + Observability
- 🟢 Status polling layer
- ⬜ Lightweight remote probes
- 🟢 Health model structs
- ⬜ Dashboard renderer

8) Config System
- ✅ TOML schema
- 🟢 Validation layer
- ⬜ Diff engine (desired vs actual)
- ⬜ Apply engine

9) Provider System
- ✅ Provider trait definitions
- 🟢 Dynamic registration
- ⬜ Capability flags
- ⬜ Provider discovery

10) TUI System
- 🟢 Global layout engine (FrankenTUI integration bootstrapped)
- 🟢 Dashboard view
- 🟢 Server list view
- 🟢 Service grid view
- 🟢 Logs view
- 🟢 Scaling panel
- ⬜ SSH terminal panel
- ⬜ Command palette

11) CLI Layer
- ✅ Clap command definitions
- 🟢 JSON output flag
- 🟢 Quiet mode
- 🟢 Exit code consistency

12) SDK Layer
- 🟢 Public Rust API
- ⬜ TS bindings generator
- 🟢 Typed command responses
- ⬜ Example automation scripts

13) State Management (Local-First)
- 🟢 State cache layer
- 🟢 Server inventory cache
- 🟢 Service registry cache
- 🟢 Drift detection

14) Project Lifecycle Commands
- 🟢 Command routing layer
- 🟢 Consistent UX semantics
- 🟢 Progress reporting

15) Error Handling + DX
- ⬜ Error taxonomy
- ⬜ Pretty error renderer
- ✅ Verbose mode
- ⬜ Retry helpers

16) Packaging + Distribution
- 🟢 Rust static builds
- ✅ npm wrapper package
- ⬜ Version sync tooling
- ⬜ Auto-update check (optional)

Current implementation focus
- Complete robust scaling + dependency-aware deploy order.
- Add machine-readable CLI output (`--json`) without breaking existing text output.
- Introduce local state cache and drift detection for idempotent reconcile loops.
- Integrate FrankenTUI as the production TUI runtime and design system.

FrankenTUI Integration Plan (Production Track)

Source of truth
- `frankentui/` git submodule from [Dicklesworthstone/frankentui](https://github.com/Dicklesworthstone/frankentui)
- Use FrankenTUI runtime, renderer, layout, widgets, and style crates as the TUI engine

Architecture decisions
- Keep `airstack-core` as orchestration/runtime engine
- Add `airstack tui` command as TUI entry point
- Build Airstack TUI app as a thin adapter on top of FrankenTUI primitives
- Keep CLI and TUI command parity (same underlying operations)

Design and UX direction
- Distinct Airstack ASCII startup banner on launch
- High-contrast but restrained palette (no noisy rainbow defaults)
- Dense operational layout: left nav + center workspace + right telemetry rail
- Smooth transitions and zero-flicker updates
- Keyboard-first workflows with command palette and context actions

Performance and reliability requirements
- Startup target: <250ms cold start for shell + initial frame render
- Frame budget target: 60fps for local interactions
- Strict one-writer discipline and deterministic render diff path (FrankenTUI)
- Avoid blocking network calls in render loop; use async state refresh workers
- Add perf baseline scripts and regression gates before feature freeze

Implementation phases
- Phase 1: integration shell (`airstack tui`, submodule wiring, launch flow) ✅
- Phase 2: reusable app shell (layout regions, nav model, status rail) ✅
- Phase 3: core views (dashboard, servers, services, logs, scale, ssh)
- Phase 4: command palette, hotkeys, and inline action workflows
- Phase 5: polish (animations, theme tuning, perf tuning, snapshot tests)

⸻

Core Philosophy
	•	Single binary, fast startup (Rust-first runtime)
	•	TUI-first UX with CLI parity
	•	Provider-agnostic abstractions
	•	Stateless control plane (local state only)
	•	Composable primitives > platform magic
	•	SDK mirrors CLI 1:1

Non-goals for v1:
	•	No Kubernetes abstraction layer
	•	No multi-region HA orchestration engines
	•	No complex schedulers
	•	No Terraform replacement ambitions

⸻

Interfaces

1. TUI (Primary Experience)

The main interface users live in daily.

Requirements
	•	Instant startup (<200ms)
	•	Keyboard-driven navigation
	•	Live updating data (logs, status, metrics)
	•	Clear visual hierarchy

Core Views
	•	Dashboard
	•	Servers
	•	Services
	•	Logs
	•	Scaling
	•	Network
	•	Providers
	•	Settings

⸻

2. CLI (Automation Layer)

Thin wrapper over the core runtime.

Requirements
	•	1:1 mapping with TUI actions
	•	Scriptable, composable output
	•	JSON output mode

Example:

airstack up
airstack scale api 3
airstack logs api --follow --json


⸻

3. SDK (Type-safe Infra Control)

Programmatic orchestration layer.

Requirements
	•	Rust native SDK
	•	Type-safe TypeScript wrapper
	•	Mirrors CLI commands
	•	No hidden behavior

⸻

Feature Scope (Tight v1)

⸻

1. Infrastructure Provisioning

Bare metal + VM provisioning abstraction.

Must Have
	•	Create servers
	•	Destroy servers
	•	List servers
	•	Server metadata inspection
	•	SSH bootstrap (keys, base setup)

Hetzner Implementation
	•	Server create/delete
	•	Region selection
	•	Server type selection
	•	SSH key upload
	•	Floating IP support

Checklist
	•	Provider trait abstraction
	•	Hetzner provider crate
	•	Idempotent provisioning logic
	•	Retry + backoff logic
	•	State reconciliation

⸻

2. Service Deployment (Containers)

Lightweight container orchestration.

Must Have
	•	Pull image
	•	Start container
	•	Stop container
	•	Restart container
	•	Remove container

Features
	•	Named services
	•	Dependency graph (simple ordering)
	•	Env vars
	•	Port mappings
	•	Volumes

Runtime Target
	•	Docker (v1 default)

Checklist
	•	Container runtime abstraction
	•	Docker runtime implementation
	•	Service lifecycle manager
	•	Dependency ordering
	•	Health status tracking

⸻

3. Scaling (Simple + Explicit)

No autoscaling engines. Manual but fast scaling.

Capabilities
	•	Horizontal scale (N replicas)
	•	Per-service scaling
	•	Quick scale hotkeys in TUI

Example:

airstack scale api 5

Checklist
	•	Replica tracking
	•	Deterministic naming
	•	Rolling spawn logic
	•	Safe scale-down logic

⸻

4. Load Balancing (Simple Layer 7)

Minimal built-in load balancing.

Approach
	•	Embedded reverse proxy (Caddy or lightweight Rust proxy)
	•	Auto-updated upstream pool

Capabilities
	•	Round-robin routing
	•	Auto-register replicas
	•	HTTP only (v1)

Checklist
	•	Embedded proxy integration
	•	Upstream pool manager
	•	Config auto-regeneration
	•	Hot reload support

⸻

5. Logs (Real-Time + Structured)

Logs are a core TUI feature.

Capabilities
	•	Stream logs live
	•	Per-service filtering
	•	Per-server filtering
	•	JSON mode (CLI)

TUI Features
	•	Scrollback buffer
	•	Colorized levels
	•	Search

Checklist
	•	Log streaming layer
	•	Multiplexed log router
	•	Persistent scrollback buffer
	•	Structured log mode

⸻

6. SSH + Remote Control

SSH deeply integrated into workflow.

Capabilities
	•	One-command SSH
	•	TUI interactive shell
	•	Multi-server selection

Example:

airstack ssh web-1

Checklist
	•	SSH connection manager
	•	Key resolution logic
	•	TUI terminal embedding
	•	Session multiplexing

⸻

7. Status + Observability (Lightweight)

Operational clarity without metrics bloat.

Metrics (Basic Only)
	•	Server online/offline
	•	CPU % (optional via agent)
	•	Memory usage
	•	Container health

TUI Dashboard
	•	Cluster overview
	•	Service health grid
	•	Replica counts

Checklist
	•	Status polling layer
	•	Lightweight remote probes
	•	Health model structs
	•	Dashboard renderer

⸻

8. Config System (TOML Source of Truth)

Declarative but simple.

Capabilities
	•	Project config
	•	Servers
	•	Services
	•	Dependencies

Behavior
	•	Config is desired state
	•	Runtime reconciles actual state

Checklist
	•	TOML schema
	•	Validation layer
	•	Diff engine (desired vs actual)
	•	Apply engine

⸻

9. Provider System (Extensible Core)

First-class plugin architecture.

Design Goals
	•	Easy provider addition
	•	Minimal surface area
	•	Stable trait contracts

Core Traits
	•	MetalProvider
	•	ContainerRuntime
	•	LoadBalancerProvider (optional)

Checklist
	•	Provider trait definitions
	•	Dynamic registration
	•	Capability flags
	•	Provider discovery

⸻

10. TUI System (Major Focus Area)

This is where Airstack wins.

Tech
	•	Ratatui / Crossterm stack
	•	Async event loop
	•	Incremental rendering

UX Requirements
	•	Zero flicker
	•	Keyboard-first navigation
	•	Contextual panels
	•	Modal workflows

Views Checklist
	•	Global layout engine
	•	Dashboard view
	•	Server list view
	•	Service grid view
	•	Logs view
	•	Scaling panel
	•	SSH terminal panel
	•	Command palette

⸻

11. CLI Layer

Thin wrapper over core engine.

Requirements
	•	Minimal logic
	•	Delegates to core runtime
	•	Machine-readable output

Checklist
	•	Clap command definitions
	•	JSON output flag
	•	Quiet mode
	•	Exit code consistency

⸻

12. SDK Layer

Programmatic usage.

Rust SDK
	•	Direct core bindings
	•	Async-first

TypeScript SDK
	•	Thin wrapper over CLI or native bindings
	•	Full type safety

Checklist
	•	Public Rust API
	•	TS bindings generator
	•	Typed command responses
	•	Example automation scripts

⸻

13. State Management (Local-First)

Avoid control-plane complexity.

Model
	•	Local state cache (~/.airstack)
	•	Remote truth from providers
	•	Reconciliation model

Checklist
	•	State cache layer
	•	Server inventory cache
	•	Service registry cache
	•	Drift detection

⸻

14. Project Lifecycle Commands

Core daily operations.

Commands
	•	init
	•	up
	•	destroy
	•	deploy
	•	scale
	•	logs
	•	ssh
	•	status

Checklist
	•	Command routing layer
	•	Consistent UX semantics
	•	Progress reporting

⸻

15. Error Handling + DX

This is a differentiator.

Principles
	•	Human-readable errors
	•	Clear remediation hints
	•	No stack trace spam by default

Checklist
	•	Error taxonomy
	•	Pretty error renderer
	•	Verbose mode
	•	Retry helpers

⸻

16. Packaging + Distribution

Frictionless install.

Goals
	•	npm install -g airstack
	•	Static binaries
	•	Multi-arch builds

Checklist
	•	Rust static builds
	•	npm wrapper package
	•	Version sync tooling
	•	Auto-update check (optional)

⸻

Nice Additions (Still v1 Safe)

These are additive but aligned with scope.
	•	Command palette in TUI
	•	JSON piping for logs
	•	SSH multi-select fanout
	•	Quick actions hotkeys (scale/logs/restart)
	•	Config diff preview before apply

⸻

Explicitly Out of Scope (v1)

Prevent scope creep.
	•	Kubernetes abstraction layer
	•	Autoscaling engines
	•	Multi-cloud mesh networking
	•	GitOps controllers
	•	Terraform replacement ambitions
	•	Distributed control planes
	•	Complex RBAC systems

⸻

Success Criteria

Airstack v1 is successful if:
	•	You can deploy infra + services in <5 minutes
	•	You can manage everything from the TUI
	•	Scaling feels instant and intuitive
	•	Logs are better than docker logs
	•	SSH feels native and frictionless
	•	Adding a provider is straightforward

⸻

Future Expansion (Post-v1)

Not part of this build, but natural evolution.
	•	Kubernetes adapter
	•	Quilt runtime integration
	•	Zero-downtime deploy strategies
	•	Built-in metrics + alerts
	•	GitOps sync mode
	•	Team collaboration

⸻

Final Guiding Principle

Airstack is not a platform.

It is a:

Fast, local, composable DevOps runtime with an elite TUI.

If something feels like platform-building, cut it.
If it feels like empowering a single operator, keep it.
