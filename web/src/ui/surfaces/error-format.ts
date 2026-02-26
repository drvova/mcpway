const ERROR_CHAIN_SEPARATOR = `\n${"─".repeat(40)}\n`;

export function safeJson(value: unknown): string {
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

export function formatErrorChain(error: unknown, depth = 0, parentMessage?: string): string {
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

export function formatFatalError(error: unknown): string {
  return formatErrorChain(error, 0);
}

export function detailsFromUnknown(input: unknown, fallbackMessage: string): { message: string; details?: string } {
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

export function detailsFromViteError(payload: unknown): { message: string; details?: string } {
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
  const detailsParts = [location, err.plugin ? `plugin: ${err.plugin}` : undefined, err.frame, err.stack].filter(Boolean);

  return {
    message,
    details: detailsParts.length > 0 ? detailsParts.join("\n\n") : undefined
  };
}
