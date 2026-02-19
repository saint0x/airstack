export interface AirstackConfig {
  project: {
    name: string;
    description?: string;
    deploy_mode?: 'local' | 'remote';
  };
  infra?: {
    servers: ServerConfig[];
  };
  services?: Record<string, ServiceConfig>;
  edge?: EdgeConfig;
}

export interface ServerConfig {
  name: string;
  provider: string;
  region: string;
  server_type: string;
  ssh_key: string;
  floating_ip?: boolean;
}

export interface ServiceConfig {
  image: string;
  ports: number[];
  env?: Record<string, string>;
  volumes?: string[];
  depends_on?: string[];
  target_server?: string;
  healthcheck?: HealthcheckConfig;
  profile?: string;
}

export interface HealthcheckConfig {
  command: string[];
  interval_secs?: number;
  retries?: number;
  timeout_secs?: number;
}

export interface EdgeConfig {
  provider: string;
  sites: EdgeSiteConfig[];
}

export interface EdgeSiteConfig {
  host: string;
  upstream_service: string;
  upstream_port: number;
  tls_email?: string;
  redirect_http?: boolean;
}

export interface DeploymentPlan {
  servers: PlannedServer[];
  services: PlannedService[];
}

export interface PlannedServer {
  name: string;
  provider: string;
  action: 'create' | 'update' | 'skip';
  config: ServerConfig;
}

export interface PlannedService {
  name: string;
  action: 'deploy' | 'update' | 'skip';
  config: ServiceConfig;
}
