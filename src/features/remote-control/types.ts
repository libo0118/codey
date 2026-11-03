export type RemoteStatus = {
  running: boolean;
  address?: string;
  urls: string[];
  port: number;
  publicUrl: string;
  accountConfigured: boolean;
  username?: string;
  revision: string;
  sessions: { id: string; createdAt: string; expiresAt: string }[];
};

export type RemoteDraft = { username: string; password: string; port: string; publicUrl: string };
