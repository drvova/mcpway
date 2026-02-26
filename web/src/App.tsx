import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";

type LogRecord = {
  ts_utc: number;
  level: string;
  target: string;
  message: string;
  mode: string;
  transport: string;
  fields: Record<string, string>;
};

type ThemePalette = {
  background: string;
  foreground: string;
  cursor: string;
  ansi: string[];
};

type ThemeDescriptor = {
  id: string;
  name: string;
  source_url: string;
  palette: ThemePalette;
};

type ThemeCatalog = {
  fetched_at_utc: number;
  themes: ThemeDescriptor[];
};

type PanelErrorKey = "logs" | "runtime" | "themes";
type ErrorScope = "global" | PanelErrorKey;
type ErrorKind = "runtime" | "promise" | "network" | "socket";

type ErrorItem = {
  id: string;
  scope: ErrorScope;
  kind: ErrorKind;
  message: string;
  details?: string;
  ts: number;
};

type OverlayError = ErrorItem & {
  signature: string;
};

const ERROR_DEDUP_WINDOW_MS = 15_000;
const ERROR_CHAIN_SEPARATOR = `\n${"─".repeat(40)}\n`;

const DEFAULT_THEME: ThemePalette = {
  background: "#101114",
  foreground: "#dde1ea",
  cursor: "#f5f7fa",
  ansi: [
    "#101114",
    "#ff5f56",
    "#27c93f",
    "#ffbd2e",
    "#4a90e2",
    "#bd93f9",
    "#00d0d0",
    "#f5f7fa",
    "#3a3f4b",
    "#ff7b72",
    "#3fb950",
    "#d29922",
    "#58a6ff",
    "#bc8cff",
    "#39c5cf",
    "#ffffff"
  ]
};

function formatTimestamp(unixSeconds: number): string {
  if (!unixSeconds) {
    return "-";
  }
  const date = new Date(unixSeconds * 1000);
  return date.toLocaleTimeString();
}

function wsEndpoint(token: string): string {
  const url = new URL("/api/logs/ws", window.location.href);
  url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  if (token) {
    url.searchParams.set("token", token);
  }
  return url.toString();
}

function apiHeaders(token: string): HeadersInit {
  if (!token) {
    return {};
  }
  return {
    Authorization: `Bearer ${token}`
  };
}

function applyTheme(theme: ThemePalette): void {
  const root = document.documentElement;
  root.style.setProperty("--bg", theme.background);
  root.style.setProperty("--fg", theme.foreground);
  root.style.setProperty("--cursor", theme.cursor);
  root.style.setProperty("--accent", theme.ansi[4] ?? "#4a90e2");
  root.style.setProperty("--muted", theme.ansi[8] ?? "#3a3f4b");
  root.style.setProperty("--ok", theme.ansi[2] ?? "#27c93f");
  root.style.setProperty("--warn", theme.ansi[3] ?? "#ffbd2e");
  root.style.setProperty("--err", theme.ansi[1] ?? "#ff5f56");
}

function detailsFromUnknown(input: unknown, fallbackMessage: string): { message: string; details?: string } {
  if (input instanceof Error) {
    return {
      message: input.message || fallbackMessage,
      details: input.stack
    };
  }

  if (typeof input === "string") {
    return {
      message: input || fallbackMessage
    };
  }

  try {
    const serialized = JSON.stringify(input);
    if (serialized && serialized !== "{}") {
      return {
        message: fallbackMessage,
        details: serialized
      };
    }
  } catch {
    // Keep fallback values.
  }

  return {
    message: fallbackMessage
  };
}

function detailsFromViteError(payload: unknown): { message: string; details?: string } {
  if (!payload || typeof payload !== "object") {
    return detailsFromUnknown(payload, "Build error");
  }

  const data = payload as {
    err?: {
      message?: string;
      stack?: string;
      frame?: string;
      id?: string;
      plugin?: string;
      loc?: {
        file?: string;
        line?: number;
        column?: number;
      };
    };
  };

  const err = data.err;
  if (!err) {
    return detailsFromUnknown(payload, "Build error");
  }

  const message = err.message?.trim() || "Build error";
  const location =
    err.loc && typeof err.loc.line === "number" && typeof err.loc.column === "number"
      ? `${err.loc.file ?? err.id ?? "unknown"}:${err.loc.line}:${err.loc.column}`
      : undefined;
  const detailsParts = [location, err.plugin ? `plugin: ${err.plugin}` : undefined, err.frame, err.stack].filter(
    Boolean
  );

  return {
    message,
    details: detailsParts.length > 0 ? detailsParts.join("\n\n") : undefined
  };
}

function safeJson(value: unknown): string {
  const seen = new WeakSet<object>();
  const json = JSON.stringify(
    value,
    (_key, part) => {
      if (typeof part === "bigint") {
        return part.toString();
      }
      if (typeof part === "object" && part) {
        if (seen.has(part)) {
          return "[Circular]";
        }
        seen.add(part);
      }
      return part;
    },
    2
  );
  return json ?? String(value);
}

function formatErrorChain(error: unknown, depth = 0, parentMessage?: string): string {
  if (!error) {
    return "Unknown error";
  }

  if (error instanceof Error) {
    const isDuplicate = depth > 0 && parentMessage === error.message;
    const prefix = depth > 0 ? `\n${ERROR_CHAIN_SEPARATOR}Caused by:\n` : "";
    const header = `${error.name}${error.message ? `: ${error.message}` : ""}`;
    const lines: string[] = [];
    const stack = error.stack?.trim();

    if (stack) {
      const startsWithHeader = stack.startsWith(header);
      if (!isDuplicate) {
        lines.push(prefix + (startsWithHeader ? stack : `${header}\n${stack}`));
      } else if (!startsWithHeader) {
        lines.push(prefix + stack);
      } else {
        const trace = stack.split("\n").slice(1).join("\n").trim();
        if (trace) {
          lines.push(prefix + trace);
        }
      }
    } else if (!isDuplicate) {
      lines.push(prefix + header);
    }

    const causedBy = "cause" in error ? (error as { cause?: unknown }).cause : undefined;
    if (causedBy) {
      const nested = formatErrorChain(causedBy, depth + 1, error.message);
      if (nested) {
        lines.push(nested);
      }
    }
    return lines.join("\n\n");
  }

  if (typeof error === "string") {
    if (depth > 0 && parentMessage === error) {
      return "";
    }
    const prefix = depth > 0 ? `\n${ERROR_CHAIN_SEPARATOR}Caused by:\n` : "";
    return prefix + error;
  }

  const prefix = depth > 0 ? `\n${ERROR_CHAIN_SEPARATOR}Caused by:\n` : "";
  return prefix + safeJson(error);
}

function formatFatalError(error: unknown): string {
  return formatErrorChain(error, 0);
}

export function App() {
  const [token, setToken] = createSignal(localStorage.getItem("mcpway:web-token") ?? "");
  const [logs, setLogs] = createSignal<LogRecord[]>([]);
  const [status, setStatus] = createSignal("connecting");
  const [runtimeHealth, setRuntimeHealth] = createSignal<string>("unknown");
  const [runtimeSessions, setRuntimeSessions] = createSignal<string[]>([]);
  const [runtimeMetrics, setRuntimeMetrics] = createSignal<Record<string, unknown>>({});
  const [themes, setThemes] = createSignal<ThemeDescriptor[]>([]);
  const [selectedThemeId, setSelectedThemeId] = createSignal(
    localStorage.getItem("mcpway:theme-id") ?? "mcpway-default"
  );
  const [search, setSearch] = createSignal("");
  const [panelErrors, setPanelErrors] = createSignal<Partial<Record<PanelErrorKey, string>>>({});
  const [overlayError, setOverlayError] = createSignal<OverlayError | null>(null);
  const [fatalError, setFatalError] = createSignal<unknown | null>(null);
  const [copiedFatalError, setCopiedFatalError] = createSignal(false);

  let socket: WebSocket | undefined;
  let errorSequence = 0;
  const errorThrottle = new Map<string, number>();

  const selectedTheme = createMemo(() => {
    const match = themes().find((theme) => theme.id === selectedThemeId());
    return match?.palette ?? DEFAULT_THEME;
  });

  createEffect(() => {
    const currentTheme = selectedTheme();
    applyTheme(currentTheme);
  });

  createEffect(() => {
    localStorage.setItem("mcpway:web-token", token());
  });

  createEffect(() => {
    localStorage.setItem("mcpway:theme-id", selectedThemeId());
  });

  const filteredLogs = createMemo(() => {
    const query = search().trim().toLowerCase();
    if (!query) {
      return logs();
    }
    return logs().filter((entry) => {
      return (
        entry.message.toLowerCase().includes(query) ||
        entry.target.toLowerCase().includes(query) ||
        entry.transport.toLowerCase().includes(query)
      );
    });
  });

  function setPanelError(panel: PanelErrorKey, message?: string): void {
    setPanelErrors((previous) => ({
      ...previous,
      [panel]: message
    }));
  }

  function raiseError(
    scope: ErrorScope,
    kind: ErrorKind,
    message: string,
    options?: {
      details?: string;
      panel?: PanelErrorKey;
      overlay?: boolean;
    }
  ): void {
    const normalized = message.trim() || "Unexpected error";
    if (options?.panel) {
      setPanelError(options.panel, normalized);
    }

    const signature = `${scope}|${kind}|${normalized}`;
    const now = Date.now();
    const next: ErrorItem = {
      id: `${now}-${++errorSequence}`,
      scope,
      kind,
      message: normalized,
      details: options?.details,
      ts: now
    };

    if (options?.overlay) {
      setOverlayError({
        ...next,
        signature
      });
    }

    const lastSeen = errorThrottle.get(signature) ?? 0;
    if (now - lastSeen < ERROR_DEDUP_WINDOW_MS) {
      return;
    }
    errorThrottle.set(signature, now);
  }

  function dismissOverlay(): void {
    setOverlayError(null);
  }

  async function copyFatalErrorDetails(): Promise<void> {
    const payload = fatalError();
    if (!payload) {
      return;
    }
    await navigator.clipboard.writeText(formatFatalError(payload));
    setCopiedFatalError(true);
    window.setTimeout(() => setCopiedFatalError(false), 2000);
  }

  function recoverFromFatalError(): void {
    setFatalError(null);
    reconnect();
    void loadThemes(false);
  }

  async function fetchJson(path: string, init: RequestInit = {}): Promise<unknown> {
    const response = await fetch(path, {
      ...init,
      headers: {
        ...(init.headers ?? {}),
        ...apiHeaders(token())
      }
    });

    const raw = await response.text();
    let parsed: unknown = null;
    if (raw) {
      try {
        parsed = JSON.parse(raw);
      } catch {
        parsed = raw;
      }
    }

    if (!response.ok) {
      const fallback = `Request failed: ${response.status}`;
      if (
        parsed &&
        typeof parsed === "object" &&
        "message" in parsed &&
        typeof (parsed as { message?: unknown }).message === "string"
      ) {
        throw new Error((parsed as { message: string }).message);
      }
      if (typeof parsed === "string" && parsed.trim()) {
        throw new Error(parsed.trim());
      }
      throw new Error(fallback);
    }

    if (parsed && typeof parsed === "object") {
      return parsed;
    }

    throw new Error("Received non-JSON response from API");
  }

  async function loadRecentLogs(): Promise<void> {
    try {
      const result = (await fetchJson("/api/logs/recent?lines=400")) as { records: LogRecord[] };
      setLogs(result.records ?? []);
      setPanelError("logs", undefined);
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to load logs");
      setStatus("auth required or API unavailable");
      raiseError("logs", "network", details.message, {
        details: details.details,
        panel: "logs",
        overlay: false
      });
    }
  }

  async function loadRuntimePanels(): Promise<void> {
    let runtimeFailure: { message: string; details?: string } | null = null;

    try {
      const health = (await fetchJson("/api/runtime/health")) as Record<string, unknown>;
      setRuntimeHealth(String(health.status ?? "unknown"));
    } catch (err) {
      setRuntimeHealth("disabled");
      runtimeFailure = detailsFromUnknown(err, "Failed to load runtime health");
    }

    try {
      const sessions = (await fetchJson("/api/runtime/sessions")) as string[];
      setRuntimeSessions(Array.isArray(sessions) ? sessions : []);
    } catch (err) {
      setRuntimeSessions([]);
      if (!runtimeFailure) {
        runtimeFailure = detailsFromUnknown(err, "Failed to load runtime sessions");
      }
    }

    try {
      const metrics = (await fetchJson("/api/runtime/metrics")) as Record<string, unknown>;
      setRuntimeMetrics(metrics);
    } catch (err) {
      setRuntimeMetrics({});
      if (!runtimeFailure) {
        runtimeFailure = detailsFromUnknown(err, "Failed to load runtime metrics");
      }
    }

    if (runtimeFailure) {
      raiseError("runtime", "network", runtimeFailure.message, {
        details: runtimeFailure.details,
        panel: "runtime",
        overlay: false
      });
    } else {
      setPanelError("runtime", undefined);
    }
  }

  async function loadThemes(forceRefresh = false): Promise<void> {
    const endpoint = forceRefresh ? "/api/themes/refresh" : "/api/themes/catalog";
    const method = forceRefresh ? "POST" : "GET";

    try {
      const catalog = (await fetchJson(endpoint, { method })) as ThemeCatalog;
      const loadedThemes = catalog.themes ?? [];
      if (loadedThemes.length > 0) {
        setThemes(loadedThemes);
        const hasSelected = loadedThemes.some((theme) => theme.id === selectedThemeId());
        if (!hasSelected) {
          setSelectedThemeId(loadedThemes[0].id);
        }
      }
      setPanelError("themes", undefined);
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to load themes");
      setThemes([
        {
          id: "mcpway-default",
          name: "MCPway Default",
          source_url: "builtin",
          palette: DEFAULT_THEME
        }
      ]);
      setSelectedThemeId("mcpway-default");
      raiseError("themes", "network", details.message, {
        details: details.details,
        panel: "themes",
        overlay: false
      });
    }
  }

  function connectLogsSocket(): void {
    if (socket) {
      socket.close();
    }

    let connected = false;
    socket = new WebSocket(wsEndpoint(token()));
    setStatus("connecting");

    socket.onopen = () => {
      connected = true;
      setStatus("live");
      setPanelError("logs", undefined);
    };

    socket.onmessage = (event) => {
      try {
        const parsed = JSON.parse(event.data) as LogRecord;
        setLogs((previous) => {
          const next = [...previous, parsed];
          if (next.length > 1500) {
            return next.slice(next.length - 1500);
          }
          return next;
        });
      } catch (err) {
        setStatus("stream parse error");
        const details = detailsFromUnknown(err, "Failed to parse log stream payload");
        raiseError("logs", "socket", details.message, {
          details: details.details,
          panel: "logs",
          overlay: true
        });
      }
    };

    socket.onclose = () => {
      setStatus("disconnected");
      if (connected) {
        raiseError("logs", "socket", "Log stream disconnected", {
          panel: "logs",
          overlay: true
        });
      }
    };

    socket.onerror = () => {
      setStatus("stream error");
      raiseError("logs", "socket", "Log stream connection error", {
        panel: "logs",
        overlay: true
      });
    };
  }

  function reconnect(): void {
    void loadRecentLogs();
    connectLogsSocket();
    void loadRuntimePanels();
  }

  function retryFromOverlay(): void {
    dismissOverlay();
    reconnect();
    void loadThemes(false);
  }

  onMount(() => {
    applyTheme(DEFAULT_THEME);
    void loadRecentLogs();
    void loadRuntimePanels();
    void loadThemes(false);
    connectLogsSocket();

    const handleWindowError = (event: ErrorEvent) => {
      const message = event.message || "Unhandled runtime error";
      const details =
        event.error instanceof Error
          ? event.error.stack
          : `${event.filename || "unknown"}:${event.lineno}:${event.colno}`;
      setFatalError(event.error ?? new Error(`${message}\n${details}`));
      raiseError("global", "runtime", message, {
        details,
        overlay: true
      });
    };

    const handleUnhandledRejection = (event: PromiseRejectionEvent) => {
      const details = detailsFromUnknown(event.reason, "Unhandled promise rejection");
      setFatalError(event.reason ?? new Error(details.message));
      raiseError("global", "promise", details.message, {
        details: details.details,
        overlay: true
      });
    };

    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && overlayError()) {
        dismissOverlay();
      }
    };

    const hotModule = import.meta.hot;
    const handleViteError = (payload: unknown) => {
      const details = detailsFromViteError(payload);
      setFatalError(new Error(details.details ? `${details.message}\n\n${details.details}` : details.message));
      raiseError("global", "runtime", details.message, {
        details: details.details,
        overlay: true
      });
    };

    const handleViteBeforeUpdate = () => {
      dismissOverlay();
    };

    const refreshInterval = window.setInterval(() => {
      void loadRuntimePanels();
    }, 5000);

    window.addEventListener("error", handleWindowError);
    window.addEventListener("unhandledrejection", handleUnhandledRejection);
    window.addEventListener("keydown", handleEscape);
    hotModule?.on("vite:error", handleViteError);
    hotModule?.on("vite:beforeUpdate", handleViteBeforeUpdate);

    onCleanup(() => {
      window.clearInterval(refreshInterval);
      socket?.close();
      window.removeEventListener("error", handleWindowError);
      window.removeEventListener("unhandledrejection", handleUnhandledRejection);
      window.removeEventListener("keydown", handleEscape);
      hotModule?.off("vite:error", handleViteError);
      hotModule?.off("vite:beforeUpdate", handleViteBeforeUpdate);
    });
  });

  return (
    <Show
      when={fatalError()}
      fallback={
        <div class="shell">
          <header class="topbar">
            <div class="topbar-copy">
              <h1>MCPway Inspector</h1>
              <p>Live logs + runtime state + themes</p>
            </div>
            <div class="topbar-actions">
              <span class={`status-badge status-${status().toLowerCase().replace(/\s+/g, "-")}`}>
                {status()}
              </span>
              <button class="action-button" type="button" onClick={() => void loadThemes(true)}>
                Refresh Themes
              </button>
              <button class="action-button" type="button" onClick={reconnect}>
                Reconnect
              </button>
            </div>
          </header>

          <main class="layout">
            <div class="layout-grid-fixed">
              <section class="panel sidebar">
                <div class="panel-header">
                  <h2>Controls</h2>
                  <span class="meta">fixed layout</span>
                </div>

                <div class="sidebar-body">
                  <div class="group">
                    <h2>Access</h2>
                    <label>
                      API Token
                      <input
                        type="password"
                        value={token()}
                        onInput={(event) => setToken(event.currentTarget.value)}
                        placeholder="optional bearer token"
                      />
                    </label>
                  </div>

                  <div class="group">
                    <h2>Search</h2>
                    <label>
                      Filter Logs
                      <input
                        type="text"
                        value={search()}
                        onInput={(event) => setSearch(event.currentTarget.value)}
                        placeholder="message / target / transport"
                      />
                    </label>
                  </div>

                  <div class="group">
                    <h2>Theme</h2>
                    <label>
                      Color Scheme
                      <select value={selectedThemeId()} onChange={(event) => setSelectedThemeId(event.currentTarget.value)}>
                        <For each={themes()}>{(theme) => <option value={theme.id}>{theme.name}</option>}</For>
                      </select>
                    </label>
                    <Show when={panelErrors().themes}>{(message) => <div class="panel-error">{message()}</div>}</Show>
                  </div>

                  <div class="group">
                    <h2>Sessions</h2>
                    <div class="sessions">
                      <Show when={runtimeSessions().length > 0} fallback={<span class="empty">none</span>}>
                        <For each={runtimeSessions()}>{(session) => <code>{session}</code>}</For>
                      </Show>
                    </div>
                  </div>
                </div>
              </section>

              <section class="panel logs">
                <div class="panel-header">
                  <h2>Logs</h2>
                  <span class="meta">{filteredLogs().length} rows</span>
                </div>

                <Show when={panelErrors().logs}>{(message) => <div class="panel-error">{message()}</div>}</Show>

                <div class="log-grid">
                  <For each={filteredLogs()}>
                    {(entry) => (
                      <article class="log-row">
                        <time class="log-time">{formatTimestamp(entry.ts_utc)}</time>
                        <strong class={`level level-${entry.level}`}>{entry.level}</strong>
                        <code class="log-source">
                          {entry.mode}:{entry.transport}
                        </code>
                        <p class="log-message">{entry.message || "(no message)"}</p>
                      </article>
                    )}
                  </For>
                </div>
              </section>

              <section class="panel metrics">
                <div class="panel-header">
                  <h2>Runtime</h2>
                  <span class="meta">{runtimeSessions().length} sessions</span>
                </div>
                <div class="metrics-body">
                  <Show when={panelErrors().runtime}>{(message) => <div class="panel-error">{message()}</div>}</Show>

                  <dl>
                    <dt>Health</dt>
                    <dd>{runtimeHealth()}</dd>
                  </dl>

                  <h2>Metrics Snapshot</h2>
                  <pre>{JSON.stringify(runtimeMetrics(), null, 2)}</pre>
                </div>
              </section>
            </div>
          </main>

          <Show when={overlayError()}>
            {(entry) => (
              <Portal>
                <div class="app-overlay" role="dialog" aria-modal="true" aria-label="Application error overlay">
                  <div class="app-overlay-backdrop" />
                  <section class="app-overlay-card">
                    <header class="app-overlay-header">
                      <h2>MCPway Runtime Error</h2>
                      <span class={`error-kind error-kind-${entry().kind}`}>{entry().kind}</span>
                    </header>
                    <p class="app-overlay-message">{entry().message}</p>
                    <Show when={entry().details}>{(details) => <pre class="app-overlay-details">{details()}</pre>}</Show>
                    <div class="app-overlay-actions">
                      <button class="action-button" type="button" onClick={retryFromOverlay}>
                        Retry
                      </button>
                      <button class="action-button" type="button" onClick={dismissOverlay}>
                        Dismiss
                      </button>
                    </div>
                  </section>
                </div>
              </Portal>
            )}
          </Show>
        </div>
      }
    >
      {(error) => (
        <section class="fatal-error-page" role="alert" aria-live="assertive">
          <div class="fatal-error-card">
            <header class="fatal-error-header">
              <h1>Application Error</h1>
              <p>A code/runtime failure was detected. Details are below.</p>
            </header>
            <textarea class="fatal-error-details" readonly value={formatFatalError(error())} />
            <div class="fatal-error-actions">
              <button class="action-button" type="button" onClick={recoverFromFatalError}>
                Retry App
              </button>
              <button class="action-button" type="button" onClick={() => void copyFatalErrorDetails()}>
                {copiedFatalError() ? "Copied" : "Copy Details"}
              </button>
            </div>
          </div>
        </section>
      )}
    </Show>
  );
}
