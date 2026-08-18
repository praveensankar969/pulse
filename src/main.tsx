import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles/tokens.css";
import "./styles/reset.css";
import "./styles/popover.css";

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
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
  </StrictMode>,
);
