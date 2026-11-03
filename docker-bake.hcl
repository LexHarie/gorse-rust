variable "VERSIONS" {
  default = "nightly"
}

variable versions {
  default = split(",", VERSIONS)
}

variable components {
  default = ["gorse-master", "gorse-server", "gorse-worker", "gorse-in-one"]
}

group "default" {
  targets = ["images"]
}

target "images" {
  matrix = {
    component = components
  }
  name       = component
  context    = "."
  dockerfile = "cmd/${component}/Dockerfile"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = [for v in versions : "zhenghaoz/${component}:${v}"]
  cache-from = ["type=gha"]
  cache-to   = ["type=gha,mode=max"]
}
