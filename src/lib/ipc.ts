import { invoke } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { CheckResult, ServiceView } from "./types";

export type FocusServicePayload = { id?: string };
export type PollerDeadPayload = { at: string };
export type OfflinePayload = { offline: boolean };

export async function listServices(): Promise<ServiceView[]> {
  return invoke<ServiceView[]>("list_services");
}

export async function checkNow(id: string): Promise<CheckResult> {
  return invoke<CheckResult>("check_now", { id });
}

export async function checkAll(): Promise<void> {
  await invoke("check_all");
}

export async function setPaused(
  id: string,
  paused: boolean,
): Promise<ServiceView> {
  return invoke<ServiceView>("set_paused", { id, paused });
}

export async function openAction(id: string): Promise<void> {
  await invoke("open_action", { id });
}

export async function quit(): Promise<void> {
  await invoke("quit");
}

export async function pollerDead(): Promise<boolean> {
  return invoke<boolean>("poller_dead");
}

export async function shouldSuppressBlur(): Promise<boolean> {
  return invoke<boolean>("should_suppress_blur");
}

export async function hidePopover(): Promise<void> {
  await getCurrentWindow().hide();
}

export async function openSettings(): Promise<void> {
  await emit("pulse://open-settings");
}

export async function openEditor(id?: string): Promise<void> {
  await emit("pulse://open-editor", id ? { id } : {});
}

export async function openDetail(id: string): Promise<void> {
  await emit("pulse://open-detail", { id });
}

export async function onServices(
  handler: (views: ServiceView[]) => void,
): Promise<UnlistenFn> {
  return listen<ServiceView[]>("pulse://services", (event) => {
    handler(event.payload);
  });
}

export async function onPollerDead(
  handler: (payload: PollerDeadPayload) => void,
): Promise<UnlistenFn> {
  return listen<PollerDeadPayload>("pulse://poller-dead", (event) => {
    handler(event.payload);
  });
}

export async function onFocusService(
  handler: (payload: FocusServicePayload) => void,
): Promise<UnlistenFn> {
  return listen<FocusServicePayload>("pulse://focus-service", (event) => {
    handler(event.payload);
  });
}

export async function onOffline(
  handler: (payload: OfflinePayload) => void,
): Promise<UnlistenFn> {
  return listen<OfflinePayload>("pulse://offline", (event) => {
    handler(event.payload);
  });
}

export function bindBlurProtocol(): Promise<UnlistenFn> {
  return getCurrentWindow().onFocusChanged(async ({ payload: focused }) => {
    if (focused) return;
    try {
      if (await shouldSuppressBlur()) return;
      await hidePopover();
    } catch {
      // Window hide is best-effort; Rust also applies the same protocol.
    }
  });
}
