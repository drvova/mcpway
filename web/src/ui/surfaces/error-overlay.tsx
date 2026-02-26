import { Dialog } from "@kobalte/core/dialog";
import { Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { Button, IconButton } from "../primitives";

export type OverlayErrorEntry = {
  kind: "runtime" | "promise" | "network" | "socket";
  message: string;
  details?: string;
  scope?: "global" | "logs" | "runtime" | "themes";
  ts?: number;
};

export interface ErrorOverlayProps {
  entry: OverlayErrorEntry | null;
  onRetry: () => void;
  onDismiss: () => void;
}

export function ErrorOverlay(props: ErrorOverlayProps) {
  const [copyState, setCopyState] = createSignal<"idle" | "copied" | "failed">("idle");
  let resetTimer: number | undefined;

  const detailText = createMemo(() => {
    const current = props.entry;
    if (!current) {
      return "";
    }
    if (current.details?.trim()) {
      return current.details.trim();
    }
    return `No stack trace or extra diagnostics were captured for this ${current.kind} error.`;
  });

  const detailPayload = createMemo(() => {
    const current = props.entry;
    if (!current) {
      return "";
    }

    const at = current.ts ? new Date(current.ts).toISOString() : new Date().toISOString();
    const scope = current.scope ?? "global";
    return [
      "MCPway Runtime Error",
      `kind: ${current.kind}`,
      `scope: ${scope}`,
      `timestamp: ${at}`,
      "",
      `message: ${current.message}`,
      "",
      "details:",
      detailText()
    ].join("\n");
  });

  createEffect(() => {
    props.entry;
    setCopyState("idle");
  });

  onCleanup(() => {
    if (resetTimer !== undefined) {
      window.clearTimeout(resetTimer);
    }
  });

  async function copyDetails() {
    const payload = detailPayload();
    if (!payload || !props.entry) {
      return;
    }

    try {
      await navigator.clipboard.writeText(payload);
      setCopyState("copied");
    } catch (error) {
      console.error("Failed to copy overlay error details", error);
      setCopyState("failed");
    }

    if (resetTimer !== undefined) {
      window.clearTimeout(resetTimer);
    }
    resetTimer = window.setTimeout(() => setCopyState("idle"), 2400);
  }

  return (
    <Dialog open={Boolean(props.entry)} onOpenChange={(isOpen) => !isOpen && props.onDismiss()}>
      <Dialog.Portal>
        <Dialog.Overlay class="app-overlay-backdrop" data-component="dialog-overlay" />
        <div class="app-overlay" data-component="dialog" data-size="normal">
          <div class="app-overlay-container" data-slot="dialog-container">
            <Dialog.Content class="app-overlay-card" data-slot="dialog-content" aria-label="Application error overlay">
              <Show when={props.entry}>
                {(entry) => (
                  <>
                    <header class="app-overlay-header" data-kind={entry().kind} data-slot="dialog-header">
                      <Dialog.Title class="app-overlay-title" data-slot="dialog-title">
                        MCPway Runtime Error
                      </Dialog.Title>
                      <div class="app-overlay-header-actions">
                        <IconButton label="Dismiss overlay" size="small" variant="ghost" onClick={props.onDismiss}>
                          ×
                        </IconButton>
                      </div>
                    </header>

                    <Dialog.Description class="app-overlay-message" data-slot="dialog-description">
                      {entry().message}
                    </Dialog.Description>

                    <dl class="app-overlay-meta">
                      <div class="app-overlay-meta-kind">
                        <dt>Kind</dt>
                        <dd>
                          <span class={`error-kind error-kind-${entry().kind}`}>{entry().kind}</span>
                        </dd>
                      </div>
                      <div>
                        <dt>Scope</dt>
                        <dd>{entry().scope ?? "global"}</dd>
                      </div>
                      <div>
                        <dt>Time</dt>
                        <dd>{entry().ts ? new Date(entry().ts).toLocaleString() : "now"}</dd>
                      </div>
                    </dl>

                    <section class="app-overlay-body" data-slot="dialog-body">
                      <h3 class="app-overlay-section-title">Details</h3>
                      <pre class="app-overlay-details">{detailText()}</pre>
                    </section>

                    <div class="app-overlay-actions">
                      <Button type="button" size="normal" variant="ghost" onClick={() => void copyDetails()}>
                        {copyState() === "copied" ? "Copied" : copyState() === "failed" ? "Copy Failed" : "Copy Details"}
                      </Button>
                      <Button type="button" size="normal" variant="secondary" onClick={props.onRetry}>
                        Retry
                      </Button>
                      <Button type="button" size="normal" variant="ghost" onClick={props.onDismiss}>
                        Dismiss
                      </Button>
                    </div>
                  </>
                )}
              </Show>
            </Dialog.Content>
          </div>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
