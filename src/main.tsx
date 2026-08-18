import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { startStore } from "./state/store";
import { Popover } from "./ui/popover/Popover";
import "./styles/tokens.css";
import "./styles/reset.css";
import "./styles/popover.css";

void startStore();

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <Popover />
  </StrictMode>,
);
