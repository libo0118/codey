import { invoke } from "../../api";
import type { RemoteStatus } from "./types";
import type { remoteSettings } from "./state";

export const remoteControlRequests = {
  status: () => invoke<RemoteStatus>("remote_control_status"),
  save: (settings: ReturnType<typeof remoteSettings>, revision: string) => invoke<RemoteStatus>("configure_remote_control", { settings, revision, confirmed: true }),
  start: () => invoke<RemoteStatus>("start_remote_control"),
  stop: () => invoke<RemoteStatus>("stop_remote_control"),
  revoke: (id: string) => invoke<RemoteStatus>("revoke_remote_session", { id }),
};
