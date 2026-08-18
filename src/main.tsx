import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startStore } from "./state/store";
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
import "./styles/tokens.css";
import "./styles/reset.css";
import "./styles/popover.css";
import "./styles/detail.css";

function surface(): "detail" | "popover" {
  try {
    if (getCurrentWindow().label === "detail") return "detail";
  } catch {
    // Vite-only preview has no Tauri window.
  }
  const params = new URLSearchParams(window.location.search);
  if (params.get("window") === "detail" || params.has("id")) return "detail";
  return "popover";
}

const kind = surface();

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    {kind === "detail" ? (
      <DetailWindow />
    ) : (
      <main className="popover">
        <header className="popover-head">
          <h1 className="wordmark">Pulse</h1>
        </header>
        <div className="empty-state">
          <p>
            Add the HTTP endpoints you own. Pulse will watch them from the tray.
          </p>
        </div>
      </main>
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
  void startStore();
}

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    {label === "settings" ? <SettingsWindow /> : <Popover />}
  </StrictMode>,
);
