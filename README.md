# macOS Service Manager

A modern, user-friendly desktop application for managing services on macOS. This tool provides a unified interface to control both Homebrew services and macOS system services (launchd/launchctl).

## Features

- 🍺 Homebrew Services Management
    - List all available Homebrew services
    - Start/Stop/Restart services
    - View service logs
    - Auto-refresh service states
    - Toggle service auto-start at login

- 🚀 LaunchCtl Services Management [TODO]
    - View system and user launch agents/daemons
    - Load/Unload services
    - Start/Stop running services
    - Monitor service status
    - View service configuration details

## Installation

[Installation instructions will be added when releases are available]

## Development

This application is built using [Tauri](https://tauri.app), combining Rust's performance with a modern web frontend.

### Prerequisites

- Rust
- Node.js
- Bun
- Xcode Command Line Tools
- Homebrew (for Homebrew services support)

### Building from Source

```bash
# Clone the repository
git clone [repository-url]
cd macos-service-manager

# Install dependencies
bun install

# Run in development mode
bun run tauri dev

# Build for production
bun run tauri build
