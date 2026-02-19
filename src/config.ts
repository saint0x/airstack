import { readFileSync } from 'fs';
import { parse as parseToml } from 'toml';
import { z } from 'zod';
import type { AirstackConfig as AirstackConfigType } from './types';

const ServerConfigSchema = z.object({
  name: z.string().min(1),
  provider: z.string().min(1),
  region: z.string().min(1),
  server_type: z.string().min(1),
  ssh_key: z.string().min(1),
  floating_ip: z.boolean().optional(),
});

const ServiceConfigSchema = z.object({
  image: z.string().min(1),
  ports: z.array(z.number().int().positive()),
  env: z.record(z.string()).optional(),
  volumes: z.array(z.string()).optional(),
  depends_on: z.array(z.string()).optional(),
  target_server: z.string().min(1).optional(),
  healthcheck: z.object({
    command: z.array(z.string()).min(1),
    interval_secs: z.number().int().positive().optional(),
    retries: z.number().int().positive().optional(),
    timeout_secs: z.number().int().positive().optional(),
  }).optional(),
  profile: z.string().optional(),
});

const AirstackConfigSchema = z.object({
  project: z.object({
    name: z.string().min(1),
    description: z.string().optional(),
    deploy_mode: z.enum(['local', 'remote']).optional(),
  }),
  infra: z.object({
    servers: z.array(ServerConfigSchema),
  }).optional(),
  services: z.record(ServiceConfigSchema).optional(),
  edge: z.object({
    provider: z.string().min(1),
    sites: z.array(z.object({
      host: z.string().min(1),
      upstream_service: z.string().min(1),
      upstream_port: z.number().int().positive(),
      tls_email: z.string().optional(),
      redirect_http: z.boolean().optional(),
    })),
  }).optional(),
});

export class AirstackConfig {
  constructor(private config: AirstackConfigType) {}

  static load(path: string): AirstackConfig {
    try {
      const content = readFileSync(path, 'utf-8');
      const parsed = parseToml(content);
      const validated = AirstackConfigSchema.parse(parsed);
      return new AirstackConfig(validated);
    } catch (error) {
      if (error instanceof z.ZodError) {
        const formattedErrors = error.errors.map(err => 
          `  ${err.path.join('.')}: ${err.message}`
        ).join('\n');
        throw new Error(`Configuration validation failed:\n${formattedErrors}`);
      }
      throw new Error(`Failed to load configuration: ${(error as Error).message}`);
    }
  }

  get project() {
    return this.config.project;
  }

  get servers() {
    return this.config.infra?.servers || [];
  }

  get services() {
    return this.config.services || {};
  }

  getService(name: string) {
    const service = this.services[name];
    if (!service) {
      throw new Error(`Service '${name}' not found in configuration`);
    }
    return service;
  }

  getServer(name: string) {
    const server = this.servers.find(s => s.name === name);
    if (!server) {
      throw new Error(`Server '${name}' not found in configuration`);
    }
    return server;
  }

  validate(): void {
    // Additional validation logic
    const serverNames = new Set<string>();
    for (const server of this.servers) {
      if (serverNames.has(server.name)) {
        throw new Error(`Duplicate server name: ${server.name}`);
      }
      serverNames.add(server.name);
    }

    const serviceNames = new Set<string>();
    for (const [serviceName, service] of Object.entries(this.services)) {
      if (serviceNames.has(serviceName)) {
        throw new Error(`Duplicate service name: ${serviceName}`);
      }
      serviceNames.add(serviceName);

      // Validate dependencies
      if (service.depends_on) {
        for (const dep of service.depends_on) {
          if (!this.services[dep]) {
            throw new Error(`Service '${serviceName}' depends on '${dep}' which is not defined`);
          }
        }
      }
    }
  }

  toJSON() {
    return this.config;
  }
}
