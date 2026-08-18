import { useSyncExternalStore } from "react";
import { sortServices } from "../lib/format";
import * as ipc from "../lib/ipc";
import type { ServiceView } from "../lib/types";

export type PulseState = {
  services: ServiceView[];
  selectedId: string | null;
  pollerDead: boolean;
  checkingIds: string[];
  now: number;
  ready: boolean;
};

const listeners = new Set<() => void>();

let state: PulseState = {
  services: [],
  selectedId: null,
  pollerDead: false,
  checkingIds: [],
  now: Date.now(),
  ready: false,
};

function emit(): void {
  for (const listener of listeners) listener();
}

function setState(patch: Partial<PulseState>): void {
  state = { ...state, ...patch };
  emit();
}

export function getState(): PulseState {
  return state;
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function usePulseStore(): PulseState {
  return useSyncExternalStore(subscribe, getState, getState);
}

function pickSelected(
  views: ServiceView[],
  preferred: string | null,
  now: number,
): string | null {
  if (preferred && views.some((view) => view.id === preferred)) return preferred;
  return sortServices(views, now)[0]?.id ?? null;
}

function applyServices(views: ServiceView[], preferred?: string | null): void {
  const now = Date.now();
  const prev = new Map(state.services.map((view) => [view.id, view]));
  const checkingIds = state.checkingIds.filter((id) => {
    const next = views.find((view) => view.id === id);
    if (!next) return false;
    return prev.get(id)?.lastCheckAt === next.lastCheckAt;
  });
  setState({
    services: views,
    checkingIds,
    selectedId: pickSelected(views, preferred ?? state.selectedId, now),
    now,
  });
}

function markChecking(ids: string[]): void {
  setState({
    checkingIds: [...new Set([...state.checkingIds, ...ids])],
  });
}

function unmarkChecking(ids: string[]): void {
  const drop = new Set(ids);
  setState({
    checkingIds: state.checkingIds.filter((id) => !drop.has(id)),
  });
}

export function selectService(id: string): void {
  if (state.services.some((view) => view.id === id)) {
    setState({ selectedId: id });
  }
}

export function selectedView(): ServiceView | undefined {
  return state.services.find((view) => view.id === state.selectedId);
}

export async function refreshServices(): Promise<void> {
  const [views, dead] = await Promise.all([ipc.listServices(), ipc.pollerDead()]);
  applyServices(views);
  setState({ pollerDead: dead });
}

export async function checkSelected(): Promise<void> {
  const id = state.selectedId;
  if (!id) return;
  markChecking([id]);
  try {
    await ipc.checkNow(id);
  } catch {
    unmarkChecking([id]);
  }
}

export async function checkEvery(): Promise<void> {
  const ids = state.services
    .filter((view) => !view.paused && view.state !== "paused")
    .map((view) => view.id);
  if (ids.length === 0) return;
  markChecking(ids);
  try {
    await ipc.checkAll();
  } catch {
    unmarkChecking(ids);
  }
}

export async function togglePauseSelected(): Promise<void> {
  const view = selectedView();
  if (!view) return;
  try {
    const next = await ipc.setPaused(view.id, !view.paused);
    applyServices(
      state.services.map((row) => (row.id === next.id ? next : row)),
      next.id,
    );
  } catch {
    // Leave the row as-is; the next pulse://services snapshot is source of truth.
  }
}

export async function openSelectedAction(): Promise<void> {
  const id = state.selectedId;
  if (!id) return;
  await ipc.openAction(id);
}

export async function openSelectedDetail(): Promise<void> {
  const id = state.selectedId;
  if (!id) return;
  await ipc.hidePopover();
  await ipc.openDetail(id);
}

export async function addService(): Promise<void> {
  await ipc.hidePopover();
  await ipc.openEditor();
}

export async function openSettings(): Promise<void> {
  await ipc.hidePopover();
  await ipc.openSettings();
}

export async function closePopover(): Promise<void> {
  await ipc.hidePopover();
}

export async function quitApp(): Promise<void> {
  await ipc.quit();
}

export async function startStore(): Promise<() => void> {
  const unlisten: Array<() => void> = [];
  try {
    await refreshServices();
  } catch {
    // Browser/vite-only: stay on the empty state.
  }
  setState({ ready: true, now: Date.now() });

  const tick = window.setInterval(() => {
    setState({ now: Date.now() });
  }, 1000);
  unlisten.push(() => window.clearInterval(tick));

  try {
    unlisten.push(
      await ipc.onServices((views) => {
        applyServices(views);
        void ipc.pollerDead().then((dead) => setState({ pollerDead: dead }));
      }),
    );
    unlisten.push(
      await ipc.onPollerDead(() => {
        setState({ pollerDead: true });
      }),
    );
    unlisten.push(
      await ipc.onFocusService((payload) => {
        if (payload.id) selectService(payload.id);
      }),
    );
    unlisten.push(await ipc.bindBlurProtocol());
  } catch {
    // Not running inside Tauri.
  }

  return () => {
    for (const stop of unlisten) stop();
  };
}
