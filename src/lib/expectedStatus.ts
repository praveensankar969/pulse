import type { ExpectedStatus } from "./types";

export const REDIRECT_HELPER =
  "We follow up to 3 redirects and evaluate the final status. Uncheck Follow redirects to treat the first response as final — required if you expect 3xx.";

export function parseExpectedStatus(raw: string): ExpectedStatus | null {
  const trimmed = raw.trim();
  if (trimmed === "2xx") return "2xx";
  if (/^\d{3}$/.test(trimmed)) {
    const code = Number(trimmed);
    return code >= 100 && code <= 599 ? code : null;
  }
  const parts = trimmed.split(/[,\s]+/).filter(Boolean);
  if (parts.length > 1 && parts.length <= 16 && parts.every((part) => /^\d{3}$/.test(part))) {
    const codes = parts.map(Number);
    if (codes.every((code) => code >= 100 && code <= 599)) return codes;
  }
  return null;
}

export function formatExpectedStatus(status: ExpectedStatus): string {
  if (status === "2xx") return "2xx";
  if (typeof status === "number") return String(status);
  return status.join(",");
}

export function expectedHas3xx(status: ExpectedStatus): boolean {
  if (status === "2xx") return false;
  if (typeof status === "number") return status >= 300 && status < 400;
  return status.some((code) => code >= 300 && code < 400);
}

export function expectedIs204(status: ExpectedStatus): boolean {
  if (status === "2xx") return false;
  if (typeof status === "number") return status === 204;
  return status.includes(204);
}
