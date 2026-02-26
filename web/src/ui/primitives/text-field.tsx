import { TextField as KobalteTextField } from "@kobalte/core/text-field";
import { Show, splitProps, type ComponentProps, type JSX } from "solid-js";

export interface TextFieldProps {
  label?: string;
  hideLabel?: boolean;
  description?: string;
  error?: string;
  variant?: "normal" | "ghost";
  value?: string;
  defaultValue?: string;
  placeholder?: string;
  type?: string;
  multiline?: boolean;
  readOnly?: boolean;
  disabled?: boolean;
  required?: boolean;
  name?: string;
  autocomplete?: string;
  onInput?: JSX.EventHandlerUnion<HTMLInputElement | HTMLTextAreaElement, InputEvent>;
  onKeyDown?: JSX.EventHandlerUnion<HTMLInputElement | HTMLTextAreaElement, KeyboardEvent>;
  class?: string;
  classList?: ComponentProps<"div">["classList"];
}

export function TextField(props: TextFieldProps) {
  const [local] = splitProps(props, [
    "label",
    "hideLabel",
    "description",
    "error",
    "variant",
    "value",
    "defaultValue",
    "placeholder",
    "type",
    "multiline",
    "readOnly",
    "disabled",
    "required",
    "name",
    "autocomplete",
    "onInput",
    "onKeyDown",
    "class",
    "classList"
  ]);

  return (
    <KobalteTextField
      data-component="input"
      data-variant={local.variant ?? "normal"}
      required={local.required}
      disabled={local.disabled}
      readOnly={local.readOnly}
      name={local.name}
      classList={{
        ...(local.classList ?? {}),
        [local.class ?? ""]: Boolean(local.class)
      }}
    >
      <Show when={local.label}>
        <KobalteTextField.Label data-slot="input-label" classList={{ "sr-only": local.hideLabel }}>
          {local.label}
        </KobalteTextField.Label>
      </Show>

      <div data-slot="input-wrapper">
        <Show
          when={local.multiline}
          fallback={
            <KobalteTextField.Input
              data-slot="input-input"
              type={local.type ?? "text"}
              value={local.value}
              defaultValue={local.defaultValue}
              placeholder={local.placeholder}
              autoComplete={local.autocomplete}
              required={local.required}
              disabled={local.disabled}
              readOnly={local.readOnly}
              name={local.name}
              onInput={local.onInput as JSX.EventHandler<HTMLInputElement, InputEvent>}
              onKeyDown={local.onKeyDown as JSX.EventHandler<HTMLInputElement, KeyboardEvent>}
            />
          }
        >
          <KobalteTextField.TextArea
            data-slot="input-input"
            value={local.value}
            defaultValue={local.defaultValue}
            placeholder={local.placeholder}
            required={local.required}
            disabled={local.disabled}
            readOnly={local.readOnly}
            name={local.name}
            onInput={local.onInput as JSX.EventHandler<HTMLTextAreaElement, InputEvent>}
            onKeyDown={local.onKeyDown as JSX.EventHandler<HTMLTextAreaElement, KeyboardEvent>}
          />
        </Show>
      </div>

      <Show when={local.description}>
        <KobalteTextField.Description data-slot="input-description">{local.description}</KobalteTextField.Description>
      </Show>
      <Show when={local.error}>
        <KobalteTextField.ErrorMessage data-slot="input-error">{local.error}</KobalteTextField.ErrorMessage>
      </Show>
    </KobalteTextField>
  );
}
