# Vendored dashboard

The Vue dashboard that the master serves at /. It is embedded with rust-embed
(src/dashboard.rs) and no build changes it.

- Module: github.com/gorse-io/dashboard
- Version: v0.5.20260714
- Hash: h1:Sy7G7Jw1JywSl5KYDSLiIYctKHBxom4qQmIn+aS1Z1c= (the go.sum line for the module, checked before extraction)
- Bundle: the zip inside statik.go, 14966149 bytes
- Script: contracts/tools/dashboard/fetch.sh (runs go run ./contracts/tools/dashboard)
- Integrity: MANIFEST.sha256 lists the hash of every other file. Check it from the
  repository root with sha256sum -c crates/gorse-master/dashboard/MANIFEST.sha256
