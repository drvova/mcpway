import { Button as KobalteButton } from "@kobalte/core/button";
import { splitProps, type ComponentProps } from "solid-js";
import type { ButtonSize, ButtonVariant } from "./button";

export interface IconButtonProps extends ComponentProps<typeof KobalteButton> {
  label: string;
  size?: ButtonSize;
  variant?: ButtonVariant;
}

export function IconButton(props: IconButtonProps) {
  const [local, rest] = splitProps(props, ["label", "size", "variant", "class", "classList"]);

  return (
    <KobalteButton
      {...rest}
      aria-label={local.label}
      title={local.label}
      data-component="icon-button"
      data-size={local.size ?? "normal"}
      data-variant={local.variant ?? "ghost"}
      classList={{
        ...(local.classList ?? {}),
        [local.class ?? ""]: Boolean(local.class)
      }}
    >
      {props.children}
    </KobalteButton>
  );
}
