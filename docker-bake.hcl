variable "VERSION" {
  default = "DEV"
}

target "common" {
  context   = "."
  dockerfile = "docker/Dockerfile"
  platforms = ["linux/amd64", "linux/arm64"]
}

target "rust" {
  inherits = ["common"]
  tags = [
    "mcpway/mcpway:latest",
    "mcpway/mcpway:${VERSION}",
    "ghcr.io/mcpway/mcpway:latest",
    "ghcr.io/mcpway/mcpway:${VERSION}"
  ]
}

group "default" {
  targets = ["rust"]
}
