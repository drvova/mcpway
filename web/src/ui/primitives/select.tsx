import { Select as KobalteSelect } from "@kobalte/core/select";
import { Show, createMemo, splitProps } from "solid-js";
import { Button, type ButtonSize, type ButtonVariant } from "./button";

export type SelectOption = {
  value: string;
  label: string;
};

export interface SelectProps {
  label?: string;
  placeholder?: string;
  options: SelectOption[];
  current?: string;
  disabled?: boolean;
  error?: string;
  size?: ButtonSize;
  variant?: ButtonVariant;
  onSelect?: (value: string | undefined) => void;
}

export function Select(props: SelectProps) {
  const [local] = splitProps(props, [
    "label",
    "placeholder",
    "options",
    "current",
    "disabled",
    "error",
    "size",
    "variant",
    "onSelect"
  ]);

  const hasOptions = createMemo(() => local.options.length > 0);
  const current = createMemo(() => local.options.find((item) => item.value === local.current));

  return (
    <div data-component="select-field">
      <Show when={local.label}>
        <label data-slot="select-label">{local.label}</label>
      </Show>

      <KobalteSelect<SelectOption>
        data-component="select"
        placement="bottom-start"
        gutter={4}
        flip
        sameWidth
        options={local.options}
        value={current()}
        optionValue={(option) => option.value}
        optionTextValue={(option) => option.label}
        disallowEmptySelection={hasOptions()}
        onChange={(value) => local.onSelect?.(value?.value)}
        itemComponent={(itemProps) => (
          <KobalteSelect.Item {...itemProps} data-slot="select-select-item">
            <KobalteSelect.ItemLabel data-slot="select-select-item-label">{itemProps.item.rawValue.label}</KobalteSelect.ItemLabel>
            <KobalteSelect.ItemIndicator data-slot="select-select-item-indicator">✓</KobalteSelect.ItemIndicator>
          </KobalteSelect.Item>
        )}
      >
        <KobalteSelect.Trigger
          as={Button}
          variant={local.variant ?? "secondary"}
          size={local.size ?? "normal"}
          disabled={local.disabled || !hasOptions()}
          data-slot="select-select-trigger"
        >
          <KobalteSelect.Value<SelectOption> data-slot="select-select-trigger-value">
            {(state) =>
              state.selectedOption()?.label ??
              current()?.label ??
              local.placeholder ??
              (hasOptions() ? "Select option" : "No options")
            }
          </KobalteSelect.Value>
          <KobalteSelect.Icon data-slot="select-select-trigger-icon">▾</KobalteSelect.Icon>
        </KobalteSelect.Trigger>

        <KobalteSelect.Portal>
          <KobalteSelect.Content data-component="select-content">
            <KobalteSelect.Listbox data-slot="select-select-content-list" />
          </KobalteSelect.Content>
        </KobalteSelect.Portal>
      </KobalteSelect>

      <Show when={local.error}>
        <p data-slot="select-error">{local.error}</p>
      </Show>
    </div>
  );
}
