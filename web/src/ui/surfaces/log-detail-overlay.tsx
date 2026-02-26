import { Dialog } from "@kobalte/core/dialog";
import { Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { Button, IconButton } from "../primitives";

type LogDetailRecord = {
  ts_utc: number;
  level: string;
  target: string;
  message: string;
  mode: string;
  transport: string;
  fields: Record<string, string>;
};

export interface LogDetailOverlayProps {
  entry: LogDetailRecord | null;
  onDismiss: () => void;
}

function formatAbsoluteTimestamp(unixSeconds: number): string {
  if (!unixSeconds) {
    return "unknown";
  }
  return new Date(unixSeconds * 1000).toLocaleString();
}

export function LogDetailOverlay(props: LogDetailOverlayProps) {
  const [copyState, setCopyState] = createSignal<"idle" | "copied" | "failed">("idle");
  let resetTimer: number | undefined;

  const messageText = createMemo(() => {
    const content = props.entry?.message ?? "";
    return content.trim() ? content : "(no message)";
  });

  const fieldsText = createMemo(() => {
    const current = props.entry;
    if (!current || Object.keys(current.fields).length === 0) {
      return "{}";
    }
    return JSON.stringify(current.fields, null, 2);
  });

  const rawPayload = createMemo(() => {
    if (!props.entry) {
      return "";
    }
    return JSON.stringify(props.entry, null, 2);
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
    if (!props.entry) {
      return;
    }

    try {
      await navigator.clipboard.writeText(rawPayload());
      setCopyState("copied");
    } catch (error) {
      console.error("Failed to copy log details", error);
      setCopyState("failed");
    }

    if (resetTimer !== undefined) {
      window.clearTimeout(resetTimer);
    }
    resetTimer = window.setTimeout(() => setCopyState("idle"), 2200);
  }

  return (
    <Dialog open={Boolean(props.entry)} onOpenChange={(isOpen) => !isOpen && props.onDismiss()}>
      <Dialog.Portal>
        <Dialog.Overlay class="app-overlay-backdrop log-detail-backdrop" data-component="dialog-overlay" />
        <div class="app-overlay log-detail-overlay" data-component="dialog" data-size="normal">
          <div class="app-overlay-container log-detail-container" data-slot="dialog-container">
            <Dialog.Content class="app-overlay-card log-detail-card" data-slot="dialog-content" aria-label="Log entry details">
              <Show when={props.entry}>
                {(entry) => (
                  <>
                    <header class="app-overlay-header log-detail-header" data-slot="dialog-header">
                      <Dialog.Title class="app-overlay-title" data-slot="dialog-title">
                        Log Entry Details
                      </Dialog.Title>
                      <div class="app-overlay-header-actions">
                        <IconButton label="Close log details" size="small" variant="ghost" onClick={props.onDismiss}>
                          ×
                        </IconButton>
                      </div>
                    </header>

                    <Dialog.Description class="app-overlay-message log-detail-message" data-slot="dialog-description">
                      {entry().target || "unknown target"}
                    </Dialog.Description>

                    <dl class="app-overlay-meta log-detail-meta">
                      <div>
                        <dt>Time</dt>
                        <dd>{formatAbsoluteTimestamp(entry().ts_utc)}</dd>
                      </div>
                      <div>
                        <dt>Level</dt>
                        <dd>
                          <span class={`level level-${entry().level}`}>{entry().level || "unknown"}</span>
                        </dd>
                      </div>
                      <div>
                        <dt>Mode</dt>
                        <dd>{entry().mode || "unknown"}</dd>
                      </div>
                      <div>
                        <dt>Transport</dt>
                        <dd>{entry().transport || "unknown"}</dd>
                      </div>
                    </dl>

                    <section class="app-overlay-body log-detail-body" data-slot="dialog-body">
                      <div class="log-detail-section">
                        <h3 class="app-overlay-section-title">Message</h3>
                        <pre class="app-overlay-details log-detail-pre log-detail-message-block">{messageText()}</pre>
                      </div>

                      <div class="log-detail-section">
                        <h3 class="app-overlay-section-title">Fields</h3>
                        <pre class="app-overlay-details log-detail-pre">{fieldsText()}</pre>
                      </div>

                      <div class="log-detail-section">
                        <h3 class="app-overlay-section-title">Raw JSON</h3>
                        <pre class="app-overlay-details log-detail-pre">{rawPayload()}</pre>
                      </div>
                    </section>

                    <div class="app-overlay-actions log-detail-actions">
                      <Button type="button" size="normal" variant="ghost" onClick={() => void copyDetails()}>
                        {copyState() === "copied" ? "Copied" : copyState() === "failed" ? "Copy Failed" : "Copy JSON"}
                      </Button>
                      <Button type="button" size="normal" variant="secondary" onClick={props.onDismiss}>
                        Close
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
