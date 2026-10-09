/**
 * Browser transport for the headless server (`PLAN.md` §12.3).
 *
 * When the page is not inside the Tauri webview, `lib/ipc.ts` sends commands to
 * `POST /api/<command>` and `lib/events.ts` reads `GET /api/events`; this module owns the
 * token and the shared response parsing. There is no other network call in the frontend.
 *
 * The token protects `/api/*` when the server listens beyond loopback. It reaches the page
 * once (`?token=…` or `#token=…`), is kept in `sessionStorage` for the rest of the tab and is
 * sent as a bearer header; the address bar is cleaned so it does not linger in history.
 */

const TOKEN_KEY = "llmtz-api-token";
const TOKEN_PATTERN = /^[A-Za-z0-9._~-]+$/;

/** The API token for this tab, if the server requires one. */
export function apiToken(): string | null {
  if (typeof window === "undefined") {
    return null;
  }
  const fromUrl = tokenFromLocation();
  if (fromUrl !== null) {
    window.sessionStorage.setItem(TOKEN_KEY, fromUrl);
    stripTokenFromLocation();
    return fromUrl;
  }
  const stored = window.sessionStorage.getItem(TOKEN_KEY);
  return stored !== null && stored.length > 0 ? stored : null;
}

/** Headers for every `/api/*` request. */
export function apiHeaders(): Record<string, string> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  const token = apiToken();
  if (token !== null) {
    headers["authorization"] = `Bearer ${token}`;
  }
  return headers;
}

/**
 * `POST /api/<command>` with the same arguments `invoke()` receives. A non-2xx response
 * rejects with the parsed `AppError` object, exactly like a rejected `invoke()`.
 */
export async function postCommand(
  command: string,
  args?: Record<string, unknown>,
): Promise<unknown> {
  const response = await fetch(`/api/${command}`, {
    method: "POST",
    headers: apiHeaders(),
    body: JSON.stringify(args ?? {}),
  });
  const text = await response.text();
  const payload = parseBody(text);
  if (!response.ok) {
    throw payload ?? new Error(`HTTP ${response.status} from ${command}`);
  }
  return payload;
}

/** Parses a JSON body, degrading to the raw text when it is not JSON. */
export function parseBody(text: string): unknown {
  if (text.length === 0) {
    return null;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

function tokenFromLocation(): string | null {
  const query = new URLSearchParams(window.location.search);
  const hash = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  const candidate = query.get("token") ?? hash.get("token");
  if (candidate === null || candidate.length === 0) {
    return null;
  }
  // A malformed token is ignored rather than stored and replayed on every request.
  return TOKEN_PATTERN.test(candidate) ? candidate : null;
}

function stripTokenFromLocation(): void {
  const url = new URL(window.location.href);
  let changed = false;
  if (url.searchParams.has("token")) {
    url.searchParams.delete("token");
    changed = true;
  }
  const hash = new URLSearchParams(url.hash.replace(/^#/, ""));
  if (hash.has("token")) {
    hash.delete("token");
    const remaining = hash.toString();
    url.hash = remaining.length > 0 ? `#${remaining}` : "";
    changed = true;
  }
  if (changed) {
    window.history.replaceState({}, "", url.toString());
  }
}
