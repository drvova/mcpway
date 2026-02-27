import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { BrandLogo } from "./ui/brand-logo";
import { Button, Card, ScrollView, Select, TextField } from "./ui/primitives";
import { detailsFromUnknown, detailsFromViteError, formatFatalError } from "./ui/surfaces/error-format";
import { ErrorOverlay } from "./ui/surfaces/error-overlay";
import { LogDetailOverlay } from "./ui/surfaces/log-detail-overlay";
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

type InspectSessionSummary = {
  session_id: string;
  session_name: string;
  transport: string;
  endpoint: string;
  status: string;
  last_error: string | null;
  connected_at_utc: number;
  updated_at_utc: number;
  history_size: number;
  notifications_size?: number;
};

type InspectToolDescriptor = {
  name: string;
  description?: string;
  input_schema: unknown;
};

type InspectHistoryEntry = {
  id: string;
  ts_utc: number;
  kind: string;
  summary: string;
  request?: unknown;
  response?: unknown;
  error?: string;
};

type InspectNotificationEntry = {
  id: string;
  ts_utc: number;
  method: string;
  summary: string;
  payload: unknown;
};

type PanelErrorKey = "logs" | "runtime" | "themes" | "inspector";
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

type InspectTab =
  | "tools"
  | "resources"
  | "prompts"
  | "ping"
  | "tasks"
  | "roots"
  | "history"
  | "notifications"
  | "runtime";

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

function safePrettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function parseStringMap(raw: string, label: string): Record<string, string> {
  const source = raw.trim() ? raw : "{}";
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch (err) {
    throw new Error(`${label} must be valid JSON: ${detailsFromUnknown(err, "Invalid JSON").message}`);
  }

  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${label} must be a JSON object`);
  }

  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (!key.trim()) {
      continue;
    }
    if (value === null || value === undefined) {
      continue;
    }
    out[key] = String(value);
  }
  return out;
}

function parseStringArray(raw: string, label: string): string[] {
  const source = raw.trim() ? raw : "[]";
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch (err) {
    throw new Error(`${label} must be valid JSON: ${detailsFromUnknown(err, "Invalid JSON").message}`);
  }

  if (!Array.isArray(parsed)) {
    throw new Error(`${label} must be a JSON array`);
  }

  return parsed.map((entry) => String(entry));
}

function parseJsonObject(raw: string, label: string): Record<string, unknown> {
  const source = raw.trim() ? raw : "{}";
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch (err) {
    throw new Error(`${label} must be valid JSON: ${detailsFromUnknown(err, "Invalid JSON").message}`);
  }

  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${label} must be a JSON object`);
  }

  return parsed as Record<string, unknown>;
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
  const [selectedLog, setSelectedLog] = createSignal<LogRecord | null>(null);
  const [logsPaused, setLogsPaused] = createSignal(false);
  const [panelErrors, setPanelErrors] = createSignal<Partial<Record<PanelErrorKey, string>>>({});
  const [overlayError, setOverlayError] = createSignal<OverlayError | null>(null);
  const [fatalError, setFatalError] = createSignal<unknown | null>(null);
  const [copiedFatalError, setCopiedFatalError] = createSignal(false);

  const [inspectSessions, setInspectSessions] = createSignal<InspectSessionSummary[]>([]);
  const [inspectSessionId, setInspectSessionId] = createSignal(localStorage.getItem("mcpway:inspect-session-id") ?? "");
  const [inspectTransport, setInspectTransport] = createSignal(localStorage.getItem("mcpway:inspect-transport") ?? "streamable-http");
  const [inspectEndpoint, setInspectEndpoint] = createSignal(localStorage.getItem("mcpway:inspect-endpoint") ?? "");
  const [inspectCommand, setInspectCommand] = createSignal(localStorage.getItem("mcpway:inspect-command") ?? "");
  const [inspectArgsText, setInspectArgsText] = createSignal(localStorage.getItem("mcpway:inspect-args") ?? "[]");
  const [inspectEnvText, setInspectEnvText] = createSignal(localStorage.getItem("mcpway:inspect-env") ?? "{}");
  const [inspectHeadersText, setInspectHeadersText] = createSignal(localStorage.getItem("mcpway:inspect-headers") ?? "{}");
  const [inspectSessionName, setInspectSessionName] = createSignal(localStorage.getItem("mcpway:inspect-session-name") ?? "");
  const [inspectTab, setInspectTab] = createSignal<InspectTab>("tools");
  const [inspectTools, setInspectTools] = createSignal<InspectToolDescriptor[]>([]);
  const [inspectToolName, setInspectToolName] = createSignal("");
  const [inspectToolArgsText, setInspectToolArgsText] = createSignal("{}");
  const [inspectPromptName, setInspectPromptName] = createSignal("");
  const [inspectPromptArgsText, setInspectPromptArgsText] = createSignal("{}");
  const [inspectResourceUri, setInspectResourceUri] = createSignal("");
  const [inspectTaskId, setInspectTaskId] = createSignal("");
  const [inspectRootsText, setInspectRootsText] = createSignal("[]");
  const [inspectHistoryEntries, setInspectHistoryEntries] = createSignal<InspectHistoryEntry[]>([]);
  const [inspectNotificationsEntries, setInspectNotificationsEntries] = createSignal<InspectNotificationEntry[]>([]);
  const [inspectResult, setInspectResult] = createSignal<unknown | null>(null);

  let socket: WebSocket | undefined;
  let errorSequence = 0;
  const errorThrottle = new Map<string, number>();

  const selectedTheme = createMemo(() => {
    const match = themes().find((theme) => theme.id === selectedThemeId());
    return match?.palette ?? DEFAULT_THEME;
  });

  const activeInspectSession = createMemo(() => {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return null;
    }
    return inspectSessions().find((session) => session.session_id === sessionId) ?? null;
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

  const inspectResultText = createMemo(() => {
    const value = inspectResult();
    if (!value) {
      return "No inspector result yet.";
    }
    return safePrettyJson(value);
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

  createEffect(() => {
    localStorage.setItem("mcpway:inspect-session-id", inspectSessionId());
    localStorage.setItem("mcpway:inspect-transport", inspectTransport());
    localStorage.setItem("mcpway:inspect-endpoint", inspectEndpoint());
    localStorage.setItem("mcpway:inspect-command", inspectCommand());
    localStorage.setItem("mcpway:inspect-args", inspectArgsText());
    localStorage.setItem("mcpway:inspect-env", inspectEnvText());
    localStorage.setItem("mcpway:inspect-headers", inspectHeadersText());
    localStorage.setItem("mcpway:inspect-session-name", inspectSessionName());
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

  function openLogDetails(entry: LogRecord): void {
    setSelectedLog(entry);
  }

  function closeLogDetails(): void {
    setSelectedLog(null);
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
    void loadInspectSessions();
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

  async function fetchInspect(path: string, payload: Record<string, unknown>): Promise<unknown> {
    return fetchJson(path, {
      method: "POST",
      headers: {
        "Content-Type": "application/json"
      },
      body: JSON.stringify(payload)
    });
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

  function disconnectLogsSocket(): void {
    if (!socket) {
      return;
    }

    socket.onopen = null;
    socket.onmessage = null;
    socket.onclose = null;
    socket.onerror = null;
    socket.close();
    socket = undefined;
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

  async function loadInspectSessions(): Promise<void> {
    try {
      const payload = (await fetchJson("/api/inspect/sessions")) as { sessions?: InspectSessionSummary[] };
      const sessions = Array.isArray(payload.sessions) ? payload.sessions : [];
      setInspectSessions(sessions);
      setInspectSessionId((previous) => {
        const current = previous.trim();
        if (current && sessions.some((session) => session.session_id === current)) {
          return current;
        }
        return sessions[0]?.session_id ?? "";
      });
      setPanelError("inspector", undefined);
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to load inspect sessions");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function connectInspectSession(): Promise<void> {
    try {
      const transport = inspectTransport();
      const headers = parseStringMap(inspectHeadersText(), "Inspect headers");
      const args = parseStringArray(inspectArgsText(), "Inspect args");
      const env = parseStringMap(inspectEnvText(), "Inspect env");

      const payload: Record<string, unknown> = {
        transport,
        session_name: inspectSessionName().trim() || undefined,
        headers,
        protocol_version: "2024-11-05",
        connect_timeout_ms: 10000,
        request_timeout_ms: 120000,
        args,
        env,
        command: inspectCommand().trim() || undefined,
        endpoint: inspectEndpoint().trim() || undefined
      };

      const response = (await fetchInspect("/api/inspect/connect", payload)) as {
        session?: InspectSessionSummary;
      };

      if (!response.session?.session_id) {
        throw new Error("Connect response did not include a session id");
      }

      setInspectSessionId(response.session.session_id);
      setInspectResult(response);
      await loadInspectSessions();
      setPanelError("inspector", undefined);
      await refreshInspectorTabData(inspectTab());
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to connect inspect session");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: true
      });
    }
  }

  async function disconnectInspectSession(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      const response = await fetchInspect("/api/inspect/disconnect", { session_id: sessionId });
      setInspectResult(response);
      setInspectToolName("");
      setInspectTools([]);
      setInspectHistoryEntries([]);
      setInspectNotificationsEntries([]);
      setInspectSessionId("");
      await loadInspectSessions();
      setPanelError("inspector", undefined);
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to disconnect inspect session");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function runInspectCall(path: string, payload: Record<string, unknown>): Promise<unknown> {
    const response = await fetchInspect(path, payload);
    setInspectResult(response);
    setPanelError("inspector", undefined);
    return response;
  }

  async function listInspectTools(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      const response = (await runInspectCall("/api/inspect/tools/list", {
        session_id: sessionId
      })) as { tools?: InspectToolDescriptor[] };
      const tools = Array.isArray(response.tools) ? response.tools : [];
      setInspectTools(tools);
      if (!inspectToolName() && tools.length > 0) {
        setInspectToolName(tools[0].name);
      }
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to list tools");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function callInspectTool(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    const toolName = inspectToolName().trim();
    if (!sessionId || !toolName) {
      return;
    }

    try {
      const argumentsPayload = parseJsonObject(inspectToolArgsText(), "Tool arguments");
      await runInspectCall("/api/inspect/tools/call", {
        session_id: sessionId,
        tool_name: toolName,
        arguments: argumentsPayload
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to call tool");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function listInspectResources(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/resources/list", {
        session_id: sessionId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to list resources");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function listInspectResourceTemplates(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/resources/templates/list", {
        session_id: sessionId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to list resource templates");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function readInspectResource(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    const uri = inspectResourceUri().trim();
    if (!sessionId || !uri) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/resources/read", {
        session_id: sessionId,
        uri
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to read resource");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function subscribeInspectResource(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    const uri = inspectResourceUri().trim();
    if (!sessionId || !uri) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/resources/subscribe", {
        session_id: sessionId,
        uri
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to subscribe resource");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function unsubscribeInspectResource(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    const uri = inspectResourceUri().trim();
    if (!sessionId || !uri) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/resources/unsubscribe", {
        session_id: sessionId,
        uri
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to unsubscribe resource");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function listInspectPrompts(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/prompts/list", {
        session_id: sessionId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to list prompts");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function getInspectPrompt(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    const name = inspectPromptName().trim();
    if (!sessionId || !name) {
      return;
    }

    try {
      const argumentsPayload = parseJsonObject(inspectPromptArgsText(), "Prompt arguments");
      await runInspectCall("/api/inspect/prompts/get", {
        session_id: sessionId,
        name,
        arguments: argumentsPayload
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to get prompt");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function pingInspectSession(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/ping", {
        session_id: sessionId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to ping inspect session");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function listInspectTasks(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/tasks/list", {
        session_id: sessionId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to list tasks");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function getInspectTask(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    const taskId = inspectTaskId().trim();
    if (!sessionId || !taskId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/tasks/get", {
        session_id: sessionId,
        task_id: taskId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to get task");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function getInspectTaskResult(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    const taskId = inspectTaskId().trim();
    if (!sessionId || !taskId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/tasks/result", {
        session_id: sessionId,
        task_id: taskId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to get task result");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function cancelInspectTask(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    const taskId = inspectTaskId().trim();
    if (!sessionId || !taskId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/tasks/cancel", {
        session_id: sessionId,
        task_id: taskId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to cancel task");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function listInspectRoots(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/roots/list", {
        session_id: sessionId
      });
      await refreshInspectHistory();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to list roots");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function setInspectRoots(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      const roots = parseStringArray(inspectRootsText(), "Roots payload").map((uri) => ({ uri }));
      await runInspectCall("/api/inspect/roots/set", {
        session_id: sessionId,
        roots
      });
      await refreshInspectHistory();
      await refreshInspectNotifications();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to set roots");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function notifyRootsChanged(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/roots/list-changed", {
        session_id: sessionId
      });
      await refreshInspectHistory();
      await refreshInspectNotifications();
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to notify roots/list_changed");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function refreshInspectHistory(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      const response = (await fetchJson(`/api/inspect/history?session_id=${encodeURIComponent(sessionId)}&limit=200`)) as {
        entries?: InspectHistoryEntry[];
      };
      setInspectHistoryEntries(Array.isArray(response.entries) ? response.entries : []);
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to load inspect history");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function clearInspectHistory(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/history/clear", {
        session_id: sessionId
      });
      setInspectHistoryEntries([]);
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to clear inspect history");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function refreshInspectNotifications(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      const response = (await fetchJson(
        `/api/inspect/notifications?session_id=${encodeURIComponent(sessionId)}&limit=200`
      )) as { entries?: InspectNotificationEntry[] };
      setInspectNotificationsEntries(Array.isArray(response.entries) ? response.entries : []);
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to load inspect notifications");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function clearInspectNotifications(): Promise<void> {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      return;
    }

    try {
      await runInspectCall("/api/inspect/notifications/clear", {
        session_id: sessionId
      });
      setInspectNotificationsEntries([]);
    } catch (err) {
      const details = detailsFromUnknown(err, "Failed to clear inspect notifications");
      raiseError("inspector", "network", details.message, {
        details: details.details,
        panel: "inspector",
        overlay: false
      });
    }
  }

  async function refreshInspectorTabData(tab: InspectTab): Promise<void> {
    switch (tab) {
      case "tools":
        await listInspectTools();
        break;
      case "history":
        await refreshInspectHistory();
        break;
      case "notifications":
        await refreshInspectNotifications();
        break;
      default:
        break;
    }
  }

  function connectLogsSocket(): void {
    if (logsPaused()) {
      return;
    }

    if (socket) {
      disconnectLogsSocket();
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
    if (!logsPaused()) {
      void loadRecentLogs();
      connectLogsSocket();
    }
    void loadRuntimePanels();
    void loadInspectSessions();
  }

  function pauseLogs(): void {
    if (logsPaused()) {
      return;
    }
    setLogsPaused(true);
    disconnectLogsSocket();
    setStatus("paused");
    setPanelError("logs", undefined);
  }

  function resumeLogs(): void {
    if (!logsPaused()) {
      return;
    }
    setLogsPaused(false);
    void loadRecentLogs();
    connectLogsSocket();
  }

  function retryFromOverlay(): void {
    dismissOverlay();
    reconnect();
    void loadThemes(false);
  }

  createEffect(() => {
    const sessionId = inspectSessionId().trim();
    if (!sessionId) {
      setInspectTools([]);
      setInspectHistoryEntries([]);
      setInspectNotificationsEntries([]);
      return;
    }
    void refreshInspectorTabData(inspectTab());
  });

  onMount(() => {
    applyTheme(DEFAULT_THEME);
    void loadRecentLogs();
    void loadRuntimePanels();
    void loadThemes(false);
    void loadInspectSessions();
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
      void loadInspectSessions();
    }, 5000);

    window.addEventListener("error", handleWindowError);
    window.addEventListener("unhandledrejection", handleUnhandledRejection);
    hotModule?.on("vite:error", handleViteError);
    hotModule?.on("vite:beforeUpdate", handleViteBeforeUpdate);

    onCleanup(() => {
      window.clearInterval(refreshInterval);
      disconnectLogsSocket();
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
              <span class={`status-badge status-${status().toLowerCase().replace(/\s+/g, "-")}`}>{status()}</span>
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
                    <h2>Inspector</h2>
                    <Select
                      label="Transport"
                      current={inspectTransport()}
                      options={[
                        { value: "streamable-http", label: "streamable-http" },
                        { value: "sse", label: "sse" },
                        { value: "ws", label: "ws" },
                        { value: "grpc", label: "grpc" },
                        { value: "stdio", label: "stdio" }
                      ]}
                      onSelect={(value) => value && setInspectTransport(value)}
                    />

                    <Show when={inspectTransport() !== "stdio"}>
                      <TextField
                        label="Endpoint"
                        type="text"
                        value={inspectEndpoint()}
                        onInput={(event) => setInspectEndpoint(event.currentTarget.value)}
                        placeholder="http://127.0.0.1:3000/mcp"
                      />
                    </Show>

                    <Show when={inspectTransport() === "stdio"}>
                      <TextField
                        label="Command"
                        type="text"
                        value={inspectCommand()}
                        onInput={(event) => setInspectCommand(event.currentTarget.value)}
                        placeholder="node"
                      />
                      <TextField
                        label="Args JSON"
                        multiline
                        value={inspectArgsText()}
                        onInput={(event) => setInspectArgsText(event.currentTarget.value)}
                        placeholder='["build/index.js"]'
                      />
                      <TextField
                        label="Env JSON"
                        multiline
                        value={inspectEnvText()}
                        onInput={(event) => setInspectEnvText(event.currentTarget.value)}
                        placeholder='{"API_KEY":"value"}'
                      />
                    </Show>

                    <TextField
                      label="Session Name"
                      type="text"
                      value={inspectSessionName()}
                      onInput={(event) => setInspectSessionName(event.currentTarget.value)}
                      placeholder="optional"
                    />
                    <TextField
                      label="Headers JSON"
                      multiline
                      value={inspectHeadersText()}
                      onInput={(event) => setInspectHeadersText(event.currentTarget.value)}
                      placeholder='{"Authorization":"Bearer ..."}'
                    />

                    <div class="button-row">
                      <Button type="button" size="small" variant="secondary" onClick={() => void connectInspectSession()}>
                        Connect
                      </Button>
                      <Button
                        type="button"
                        size="small"
                        variant="ghost"
                        disabled={!inspectSessionId().trim()}
                        onClick={() => void disconnectInspectSession()}
                      >
                        Disconnect
                      </Button>
                      <Button type="button" size="small" variant="ghost" onClick={() => void loadInspectSessions()}>
                        Refresh
                      </Button>
                    </div>

                    <Select
                      label="Active Session"
                      current={inspectSessionId()}
                      options={inspectSessions().map((session) => ({
                        value: session.session_id,
                        label: `${session.session_name} (${session.status})`
                      }))}
                      onSelect={(value) => setInspectSessionId(value ?? "")}
                    />

                    <Show when={activeInspectSession()}>
                      {(session) => (
                        <Card class="inspector-session-card">
                          <div class="inspector-session-title">{session().session_name}</div>
                          <div class="inspector-session-meta">{session().transport}</div>
                          <div class="inspector-session-meta">{session().endpoint}</div>
                        </Card>
                      )}
                    </Show>
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
                    <h2>Runtime Sessions</h2>
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
                  <div class="panel-header-actions">
                    <span class="meta">{filteredLogs().length} rows</span>
                    <Show
                      when={!logsPaused()}
                      fallback={
                        <Button type="button" size="small" variant="secondary" onClick={resumeLogs}>
                          Resume
                        </Button>
                      }
                    >
                      <Button type="button" size="small" variant="ghost" onClick={pauseLogs}>
                        Pause
                      </Button>
                    </Show>
                  </div>
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
                      <article
                        class="log-row"
                        role="button"
                        tabIndex={0}
                        aria-label={`Inspect log ${entry.level} ${entry.mode} ${entry.transport}`}
                        onClick={() => openLogDetails(entry)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" || event.key === " ") {
                            event.preventDefault();
                            openLogDetails(entry);
                          }
                        }}
                      >
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

              <section class="panel metrics inspector-panel">
                <div class="panel-header">
                  <h2>Inspector</h2>
                  <span class="meta">{activeInspectSession()?.status ?? "no session"}</span>
                </div>
                <div class="metrics-body inspector-body">
                  <Show when={panelErrors().inspector}>
                    {(message) => (
                      <Card variant="error" class="panel-error">
                        {message()}
                      </Card>
                    )}
                  </Show>

                  <Show when={activeInspectSession()} fallback={<Card class="inspector-empty">Connect an inspect session to use tabs.</Card>}>
                    <div class="inspector-tabs">
                      <For each={["tools", "resources", "prompts", "ping", "tasks", "roots", "history", "notifications", "runtime"] as const}>
                        {(tab) => (
                          <Button
                            type="button"
                            size="small"
                            variant={inspectTab() === tab ? "secondary" : "ghost"}
                            onClick={() => {
                              setInspectTab(tab);
                              void refreshInspectorTabData(tab);
                            }}
                          >
                            {tab}
                          </Button>
                        )}
                      </For>
                    </div>

                    <Show when={inspectTab() === "tools"}>
                      <div class="inspector-section">
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void listInspectTools()}>
                            List Tools
                          </Button>
                        </div>
                        <Select
                          label="Tool"
                          current={inspectToolName()}
                          options={inspectTools().map((tool) => ({ value: tool.name, label: tool.name }))}
                          onSelect={(value) => setInspectToolName(value ?? "")}
                        />
                        <TextField
                          label="Arguments JSON"
                          multiline
                          value={inspectToolArgsText()}
                          onInput={(event) => setInspectToolArgsText(event.currentTarget.value)}
                        />
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void callInspectTool()}>
                            Call Tool
                          </Button>
                        </div>
                      </div>
                    </Show>

                    <Show when={inspectTab() === "resources"}>
                      <div class="inspector-section">
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void listInspectResources()}>
                            List Resources
                          </Button>
                          <Button type="button" size="small" variant="ghost" onClick={() => void listInspectResourceTemplates()}>
                            List Templates
                          </Button>
                        </div>
                        <TextField
                          label="Resource URI"
                          type="text"
                          value={inspectResourceUri()}
                          onInput={(event) => setInspectResourceUri(event.currentTarget.value)}
                          placeholder="file:///..."
                        />
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void readInspectResource()}>
                            Read
                          </Button>
                          <Button type="button" size="small" variant="ghost" onClick={() => void subscribeInspectResource()}>
                            Subscribe
                          </Button>
                          <Button type="button" size="small" variant="ghost" onClick={() => void unsubscribeInspectResource()}>
                            Unsubscribe
                          </Button>
                        </div>
                      </div>
                    </Show>

                    <Show when={inspectTab() === "prompts"}>
                      <div class="inspector-section">
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void listInspectPrompts()}>
                            List Prompts
                          </Button>
                        </div>
                        <TextField
                          label="Prompt Name"
                          type="text"
                          value={inspectPromptName()}
                          onInput={(event) => setInspectPromptName(event.currentTarget.value)}
                        />
                        <TextField
                          label="Prompt Args JSON"
                          multiline
                          value={inspectPromptArgsText()}
                          onInput={(event) => setInspectPromptArgsText(event.currentTarget.value)}
                        />
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void getInspectPrompt()}>
                            Get Prompt
                          </Button>
                        </div>
                      </div>
                    </Show>

                    <Show when={inspectTab() === "ping"}>
                      <div class="inspector-section">
                        <Button type="button" size="small" variant="secondary" onClick={() => void pingInspectSession()}>
                          Ping
                        </Button>
                      </div>
                    </Show>

                    <Show when={inspectTab() === "tasks"}>
                      <div class="inspector-section">
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void listInspectTasks()}>
                            List Tasks
                          </Button>
                        </div>
                        <TextField
                          label="Task ID"
                          type="text"
                          value={inspectTaskId()}
                          onInput={(event) => setInspectTaskId(event.currentTarget.value)}
                        />
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void getInspectTask()}>
                            Get
                          </Button>
                          <Button type="button" size="small" variant="ghost" onClick={() => void getInspectTaskResult()}>
                            Result
                          </Button>
                          <Button type="button" size="small" variant="ghost" onClick={() => void cancelInspectTask()}>
                            Cancel
                          </Button>
                        </div>
                      </div>
                    </Show>

                    <Show when={inspectTab() === "roots"}>
                      <div class="inspector-section">
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void listInspectRoots()}>
                            List Roots
                          </Button>
                        </div>
                        <TextField
                          label="Roots JSON Array"
                          multiline
                          value={inspectRootsText()}
                          onInput={(event) => setInspectRootsText(event.currentTarget.value)}
                          placeholder='["file:///workspace"]'
                        />
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void setInspectRoots()}>
                            Set Roots
                          </Button>
                          <Button type="button" size="small" variant="ghost" onClick={() => void notifyRootsChanged()}>
                            Notify Changed
                          </Button>
                        </div>
                      </div>
                    </Show>

                    <Show when={inspectTab() === "history"}>
                      <div class="inspector-section">
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void refreshInspectHistory()}>
                            Refresh History
                          </Button>
                          <Button type="button" size="small" variant="ghost" onClick={() => void clearInspectHistory()}>
                            Clear
                          </Button>
                        </div>
                        <ScrollView class="inspector-list">
                          <For each={inspectHistoryEntries()}>
                            {(entry) => (
                              <article class="inspector-list-item">
                                <div class="inspector-list-item-title">{entry.kind}</div>
                                <div class="inspector-list-item-meta">{formatTimestamp(entry.ts_utc)}</div>
                                <p>{entry.summary}</p>
                                <Show when={entry.error}>
                                  {(error) => <p class="inspector-error-text">{error()}</p>}
                                </Show>
                              </article>
                            )}
                          </For>
                        </ScrollView>
                      </div>
                    </Show>

                    <Show when={inspectTab() === "notifications"}>
                      <div class="inspector-section">
                        <div class="button-row">
                          <Button type="button" size="small" variant="secondary" onClick={() => void refreshInspectNotifications()}>
                            Refresh Notifications
                          </Button>
                          <Button type="button" size="small" variant="ghost" onClick={() => void clearInspectNotifications()}>
                            Clear
                          </Button>
                        </div>
                        <ScrollView class="inspector-list">
                          <For each={inspectNotificationsEntries()}>
                            {(entry) => (
                              <article class="inspector-list-item">
                                <div class="inspector-list-item-title">{entry.method}</div>
                                <div class="inspector-list-item-meta">{formatTimestamp(entry.ts_utc)}</div>
                                <p>{entry.summary}</p>
                              </article>
                            )}
                          </For>
                        </ScrollView>
                      </div>
                    </Show>

                    <Show when={inspectTab() === "runtime"}>
                      <div class="inspector-section">
                        <dl>
                          <dt>Health</dt>
                          <dd>{runtimeHealth()}</dd>
                        </dl>
                        <h2>Runtime Metrics</h2>
                        <ScrollView class="metrics-scroll">
                          <pre>{JSON.stringify(runtimeMetrics(), null, 2)}</pre>
                        </ScrollView>
                      </div>
                    </Show>

                    <h2>Result</h2>
                    <ScrollView class="metrics-scroll inspector-result-scroll">
                      <pre>{inspectResultText()}</pre>
                    </ScrollView>
                  </Show>
                </div>
              </section>
            </div>
          </main>

          <LogDetailOverlay entry={selectedLog()} onDismiss={closeLogDetails} />
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
