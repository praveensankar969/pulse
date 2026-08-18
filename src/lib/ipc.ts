import { invoke } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  AppSettings,
  CheckEvidence,
  CheckResult,
  ServiceDraft,
  ServiceView,
} from "./types";
import type { AppSettings, CheckResult, ServiceView } from "./types";

export type FocusServicePayload = { id?: string };
export type PollerDeadPayload = { at: string };
export type OfflinePayload = { offline: boolean };

export async function listServices(): Promise<ServiceView[]> {
  return invoke<ServiceView[]>("list_services");
import type {
  AppSettings,
  CheckResult,
  DetailPayload,
  ServiceView,
} from "./types";

export type BeginReveal = { token: string; ttlMs: number };

export async function listServices(): Promise<ServiceView[]> {
  return invoke<ServiceView[]>("list_services");
}

export async function getDetail(id: string): Promise<DetailPayload> {
  return invoke<DetailPayload>("get_detail", { id });
}

export async function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export async function updateSettings(
  settings: AppSettings,
): Promise<AppSettings> {
  return invoke<AppSettings>("update_settings", { settings });
}

export async function openSettings(): Promise<void> {
  try {
    await invoke("open_settings");
  } catch {
    await emit("pulse://open-settings");
  }
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

export async function snooze(
  id: string,
  until: string | null,
): Promise<ServiceView> {
  return invoke<ServiceView>("snooze", { id, until });
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
  await invoke("open_settings");
}

export async function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export async function updateSettings(settings: AppSettings): Promise<AppSettings> {
  return invoke<AppSettings>("update_settings", { settings });
}

export async function maybeAskLaunchAtLogin(): Promise<AppSettings> {
  return invoke<AppSettings>("maybe_ask_launch_at_login");
}

export async function pendingLaunchPrompt(): Promise<boolean> {
  return invoke<boolean>("pending_launch_prompt");
}

export async function answerLaunchPrompt(enable: boolean): Promise<AppSettings> {
  return invoke<AppSettings>("answer_launch_prompt", { enable });
}

export type ImportResult = { added: number; updated: number };

export async function exportConfig(opts: {
  includeSecrets: boolean;
  includeSettings?: boolean;
}): Promise<string> {
  return invoke<string>("export_config", opts);
}

export async function importConfig(opts: {
  includeSecrets: boolean;
  replaceSettings?: boolean;
}): Promise<ImportResult> {
  return invoke<ImportResult>("import_config", opts);
}

export async function resetAll(): Promise<void> {
  await invoke("reset_all");
}

export function isCanceled(cause: unknown): boolean {
  const message = cause instanceof Error ? cause.message : String(cause);
  return message === "canceled" || message.endsWith("canceled");
}

export async function openEditor(id?: string): Promise<void> {
  await invoke("open_editor", { id: id ?? null });
}

export async function closeEditor(): Promise<void> {
  await invoke("close_editor");
}

export async function openDetail(id: string): Promise<void> {
  await emit("pulse://open-detail", { id });
}

export async function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export async function saveService(draft: ServiceDraft): Promise<ServiceView> {
  return invoke<ServiceView>("save_service", { draft });
}

export async function testDraft(draft: ServiceDraft): Promise<CheckEvidence> {
  return invoke<CheckEvidence>("test_draft", { draft });
}

export async function onEditorTarget(
  handler: (payload: { id?: string }) => void,
): Promise<UnlistenFn> {
  return listen<{ id?: string }>("pulse://editor-target", (event) => {
    handler(event.payload);
  });
export async function openDetail(id: string): Promise<void> {
  try {
    await invoke("open_detail", { id });
  } catch {
    await emit("pulse://open-detail", { id });
  }
}

export async function openEditor(id: string): Promise<void> {
  await emit("pulse://open-editor", { id });
}

export async function beginReveal(
  id: string,
  headerKey: string,
): Promise<BeginReveal> {
  return invoke<BeginReveal>("begin_reveal", { id, headerKey });
}

export async function revealSecret(
  id: string,
  headerKey: string,
  token: string,
): Promise<string> {
  return invoke<string>("reveal_secret", { id, headerKey, token });
}

export async function endReveal(token: string): Promise<void> {
  await invoke("end_reveal", { token });
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

export async function onSettings(
  handler: (settings: AppSettings) => void,
): Promise<UnlistenFn> {
  return listen<AppSettings>("pulse://settings", (event) => {
    handler(event.payload);
  });
}

export async function onAskLaunchAtLogin(handler: () => void): Promise<UnlistenFn> {
  return listen("pulse://ask-launch-at-login", () => {
    handler();
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
export async function onDetailService(
  handler: (id: string) => void,
): Promise<UnlistenFn> {
  return listen<{ id: string }>("pulse://detail-service", (event) => {
    if (event.payload.id) handler(event.payload.id);
  });
}

export async function onSettings(
  handler: (settings: AppSettings) => void,
): Promise<UnlistenFn> {
  return listen<AppSettings>("pulse://settings", (event) => {
    handler(event.payload);
  });
}
