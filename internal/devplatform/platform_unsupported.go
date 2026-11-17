//go:build !darwin || !arm64

package devplatform

// Devbase manages arm64 software on Apple Silicon Macs only. Refusing to
// compile anywhere else keeps that a build-time fact rather than a runtime
// check: an arm64 macOS binary always runs on Apple Silicon, since Rosetta
// translates only Intel code.
var _ = Devbase_builds_only_for_darwin_arm64
