import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startStore } from "./state/store";
import { DetailWindow } from "./ui/detail/DetailWindow";
import { EditorWindow } from "./ui/editor/EditorWindow";
import { Popover } from "./ui/popover/Popover";
import { SettingsWindow } from "./ui/settings/SettingsWindow";
import "./styles/tokens.css";
import "./styles/reset.css";
import "./styles/popover.css";
import "./styles/editor.css";

function surface(): "popover" | "editor" {
  try {
    if (getCurrentWindow().label === "editor") return "editor";
  } catch {
    // Vite-only / tests.
  }
  return new URLSearchParams(window.location.search).get("surface") === "editor"
    ? "editor"
    : "popover";
}

const kind = surface();
document.documentElement.classList.add(kind);
if (kind === "popover") {
  void startStore();
}

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>{kind === "editor" ? <EditorWindow /> : <Popover />}</StrictMode>,
import { DetailWindow } from "./ui/detail/DetailWindow";
import { PopoverWindow } from "./ui/popover/PopoverWindow";
import { SettingsWindow } from "./ui/settings/SettingsWindow";
import "./styles/tokens.css";
import "./styles/reset.css";
import "./styles/popover.css";
import "./styles/detail.css";
import "./styles/editor.css";
import "./styles/settings.css";

function surface(): "popover" | "editor" | "detail" | "settings" {
  try {
    const label = getCurrentWindow().label;
    if (label === "editor") return "editor";
    if (label === "detail") return "detail";
    if (label === "settings") return "settings";
  } catch {
    // Vite-only preview / tests have no Tauri window.
  }
  const params = new URLSearchParams(window.location.search);
  if (params.get("surface") === "editor") return "editor";
  if (params.get("window") === "settings") return "settings";
  if (params.get("window") === "detail" || params.has("id")) return "detail";
  return "popover";
}

const kind = surface();

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    {kind === "detail" ? (
      <DetailWindow />
    ) : kind === "settings" ? (
      <SettingsWindow />
    ) : (
      <PopoverWindow />
    )}
import "./styles/settings.css";

function windowLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return "popover";
  }
}

const label = windowLabel();
if (label !== "settings") {
document.documentElement.classList.add(kind);
if (kind === "popover") {
  void startStore();
}

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    {label === "settings" ? <SettingsWindow /> : <Popover />}
    {kind === "editor" ? (
      <EditorWindow />
    ) : kind === "detail" ? (
      <DetailWindow />
    ) : kind === "settings" ? (
      <SettingsWindow />
    ) : (
      <Popover />
    )}
  </StrictMode>,
);
