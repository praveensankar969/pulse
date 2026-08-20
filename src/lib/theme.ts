/** Product chrome is always the landing light sheet. Stored theme is ignored. */
export function applyTheme(_theme?: unknown): void {
  document.documentElement.dataset.theme = "light";
}
