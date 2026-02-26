import { Card, Button, TextField } from "../primitives";
import { BrandLogo } from "../brand-logo";
import { formatFatalError } from "./error-format";

export interface ErrorScreenProps {
  error: unknown;
  copied: boolean;
  onRetry: () => void;
  onCopy: () => void | Promise<void>;
}

export function ErrorScreen(props: ErrorScreenProps) {
  return (
    <section class="fatal-error-page" role="alert" aria-live="assertive">
      <Card class="fatal-error-card">
        <BrandLogo class="fatal-error-logo" alt="MCPway" />
        <header class="fatal-error-header">
          <h1>Application Error</h1>
          <p>A code/runtime failure was detected. Details are below.</p>
        </header>
        <TextField
          class="fatal-error-field"
          label="Error details"
          hideLabel
          multiline
          readOnly
          value={formatFatalError(props.error)}
        />
        <div class="fatal-error-actions">
          <Button type="button" variant="primary" size="large" onClick={props.onRetry}>
            Retry App
          </Button>
          <Button type="button" variant="secondary" size="large" onClick={() => void props.onCopy()}>
            {props.copied ? "Copied" : "Copy Details"}
          </Button>
        </div>
      </Card>
    </section>
  );
}
