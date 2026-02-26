import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { BrandLogo } from "./ui/brand-logo";
import { Button, Card, ScrollView, Select, TextField } from "./ui/primitives";
import { detailsFromUnknown, detailsFromViteError, formatFatalError } from "./ui/surfaces/error-format";
import { ErrorOverlay } from "./ui/surfaces/error-overlay";
import { ErrorScreen } from "./ui/surfaces/error-screen";

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
    hotModule?.on("vite:error", handleViteError);
    hotModule?.on("vite:beforeUpdate", handleViteBeforeUpdate);

    onCleanup(() => {
      window.clearInterval(refreshInterval);
      socket?.close();
      window.removeEventListener("error", handleWindowError);
      window.removeEventListener("unhandledrejection", handleUnhandledRejection);
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
              <h1 class="sr-only">MCPway Inspector</h1>
              <div class="topbar-brand-inline">
                <BrandLogo class="topbar-logo" alt="MCPway" />
              </div>
            </div>
            <div class="topbar-actions">
              <span class={`status-badge status-${status().toLowerCase().replace(/\s+/g, "-")}`}>
                {status()}
              </span>
              <Button type="button" size="normal" variant="secondary" onClick={() => void loadThemes(true)}>
                Refresh Themes
              </Button>
              <Button type="button" size="normal" variant="secondary" onClick={reconnect}>
                Reconnect
              </Button>
            </div>
          </header>

          <main class="layout">
            <div class="layout-grid-fixed">
              <section class="panel sidebar">
                <div class="panel-header">
                  <h2>Controls</h2>
                </div>

                <div class="sidebar-body">
                  <div class="group">
                    <h2>Access</h2>
                    <TextField
                      label="API Token"
                      type="text"
                      value={token()}
                      onInput={(event) => setToken(event.currentTarget.value)}
                      placeholder="optional bearer token"
                    />
                  </div>

                  <div class="group">
                    <h2>Search</h2>
                    <TextField
                      label="Filter Logs"
                      type="text"
                      value={search()}
                      onInput={(event) => setSearch(event.currentTarget.value)}
                      placeholder="message / target / transport"
                    />
                  </div>

                  <div class="group">
                    <h2>Theme</h2>
                    <Select
                      label="Color Scheme"
                      current={selectedThemeId()}
                      options={themes().map((theme) => ({ value: theme.id, label: theme.name }))}
                      onSelect={(value) => value && setSelectedThemeId(value)}
                      error={panelErrors().themes}
                    />
                  </div>

                  <div class="group">
                    <h2>Sessions</h2>
                    <ScrollView class="sessions">
                      <Show when={runtimeSessions().length > 0} fallback={<span class="empty">none</span>}>
                        <For each={runtimeSessions()}>{(session) => <code>{session}</code>}</For>
                      </Show>
                    </ScrollView>
                  </div>
                </div>
              </section>

              <section class="panel logs">
                <div class="panel-header">
                  <h2>Logs</h2>
                  <span class="meta">{filteredLogs().length} rows</span>
                </div>

                <Show when={panelErrors().logs}>
                  {(message) => (
                    <Card variant="error" class="panel-error">
                      {message()}
                    </Card>
                  )}
                </Show>

                <ScrollView class="log-grid">
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
                </ScrollView>
              </section>

              <section class="panel metrics">
                <div class="panel-header">
                  <h2>Runtime</h2>
                  <span class="meta">{runtimeSessions().length} sessions</span>
                </div>
                <div class="metrics-body">
                  <Show when={panelErrors().runtime}>
                    {(message) => (
                      <Card variant="error" class="panel-error">
                        {message()}
                      </Card>
                    )}
                  </Show>

                  <dl>
                    <dt>Health</dt>
                    <dd>{runtimeHealth()}</dd>
                  </dl>

                  <h2>Metrics Snapshot</h2>
                  <ScrollView class="metrics-scroll">
                    <pre>{JSON.stringify(runtimeMetrics(), null, 2)}</pre>
                  </ScrollView>
                </div>
              </section>
            </div>
          </main>

          <ErrorOverlay entry={overlayError()} onRetry={retryFromOverlay} onDismiss={dismissOverlay} />
        </div>
      }
    >
      {(error) => (
        <ErrorScreen
          error={error()}
          copied={copiedFatalError()}
          onRetry={recoverFromFatalError}
          onCopy={() => void copyFatalErrorDetails()}
        />
      )}
    </Show>
  );
}
