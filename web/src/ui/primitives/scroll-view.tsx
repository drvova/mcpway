import { splitProps, type ComponentProps } from "solid-js";

export interface ScrollViewProps extends ComponentProps<"div"> {}

export function ScrollView(props: ScrollViewProps) {
  const [local, rest] = splitProps(props, ["class", "classList"]);

  return (
    <div
      {...rest}
      data-component="scroll-view"
      classList={{
        ...(local.classList ?? {}),
        [local.class ?? ""]: Boolean(local.class)
      }}
    >
      {props.children}
    </div>
  );
}
