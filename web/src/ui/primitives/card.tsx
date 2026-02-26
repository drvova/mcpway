import { splitProps, type ComponentProps } from "solid-js";

export interface CardProps extends ComponentProps<"div"> {
  variant?: "normal" | "error" | "warning" | "success" | "info";
}

export function Card(props: CardProps) {
  const [local, rest] = splitProps(props, ["variant", "class", "classList"]);

  return (
    <div
      {...rest}
      data-component="card"
      data-variant={local.variant ?? "normal"}
      classList={{
        ...(local.classList ?? {}),
        [local.class ?? ""]: Boolean(local.class)
      }}
    >
      {props.children}
    </div>
  );
}
