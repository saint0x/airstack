export interface AirstackConfig {
  project: {
    name: string;
    description?: string;
  };
  infra?: {
    servers: ServerConfig[];
  };
  services?: Record<string, ServiceConfig>;
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