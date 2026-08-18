import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startStore } from "./state/store";
import { EditorWindow } from "./ui/editor/EditorWindow";
import { Popover } from "./ui/popover/Popover";
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
);
