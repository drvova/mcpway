import logoUrl from "../assets/mcpway-ascii-logo.svg";

export interface BrandLogoProps {
  class?: string;
  alt?: string;
}

export function BrandLogo(props: BrandLogoProps) {
  return (
    <img
      src={logoUrl}
      alt={props.alt ?? "MCPway"}
      class={props.class}
      loading="eager"
      decoding="async"
      draggable={false}
    />
  );
}
