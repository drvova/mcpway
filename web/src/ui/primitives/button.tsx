import { Button as KobalteButton } from "@kobalte/core/button";
import { splitProps, type ComponentProps } from "solid-js";

export type ButtonVariant = "primary" | "secondary" | "ghost";
export type ButtonSize = "small" | "normal" | "large";

export type ButtonProps = ComponentProps<typeof KobalteButton> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
};

export function Button(props: ButtonProps) {
  const [local, rest] = splitProps(props, ["variant", "size", "class", "classList"]);

  return (
    <KobalteButton
      {...rest}
      data-component="button"
      data-size={local.size ?? "normal"}
      data-variant={local.variant ?? "secondary"}
      classList={{
        ...(local.classList ?? {}),
        [local.class ?? ""]: Boolean(local.class)
      }}
    >
      {props.children}
    </KobalteButton>
  );
}
