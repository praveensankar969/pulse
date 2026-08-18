import type { Theme } from "./types";

let current: Theme = "system";
let media: MediaQueryList | null = null;

function resolved(theme: Theme): "dark" | "light" {
  if (theme === "system") {
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }
  return theme;
}

function paint(theme: Theme): void {
  document.documentElement.dataset.theme = resolved(theme);
}

function onSystemChange(): void {
  if (current === "system") paint("system");
}

export function applyTheme(theme: Theme): void {
  current = theme;
  paint(theme);
  if (typeof window === "undefined") return;
  if (!media) {
    media = window.matchMedia("(prefers-color-scheme: dark)");
    media.addEventListener("change", onSystemChange);
  }
}
