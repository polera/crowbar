```
  ▄████▄  ██▀███   ▒█████   █     █░ ▄▄▄▄    ▄▄▄       ██▀███
 ▒██▀ ▀█ ▓██ ▒ ██▒▒██▒  ██▒▓█░ █ ░█░▓█████▄ ▒████▄    ▓██ ▒ ██▒
 ▒▓█    ▄▓██ ░▄█ ▒░▒██░  ██▒▒█░ █ ░█ ▒██▒ ▄██▒██  ▀█▄  ▓██ ░▄█ ▒
 ▒▓▓▄ ▄██▒██▀▀█▄  ░▒██   ██░░█░ █ ░█ ▒██░█▀  ░██▄▄▄▄██ ▒██▀▀█▄
 ▒ ▓███▀ ░██▓ ▒██▒ ░████▓▒░░░██▒██▓  ░▓█  ▀█▓ ▓█   ▓██▒░██▓ ▒██▒
 ░ ░▒ ▒  ░ ▒▓ ░▒▓░ ░▒░▒░▒░  ░ ▓░▒ ▒  ░▒▓███▀▒ ▒▒   ▓▒█░░ ▒▓ ░▒▓░
   ░  ▒    ░▒ ░ ▒░ ░░▒░ ░   ░ ░▒░ ░  ▒░▒   ░   ▒   ▒▒ ░  ░▒ ░ ▒░
          ░░   ░   ░░░          ░░              ░   ▒     ░░   ░
           ░       ░            ░               ░   ░      ░

            ▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬
            ░▒▓  W E B  S E C U R I T Y  P R O X Y  ▓▒░
            ▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬
                    ··· P R Y   O P E N ···
                    ···  T H E   W E B  ···
```

# Crowbar

A terminal-based web security testing proxy built in Rust. Intercept, inspect, and modify HTTP/HTTPS traffic through an interactive TUI — similar in spirit to Burp Suite or OWASP ZAP, but running entirely in your terminal.

## Features

- **MITM Proxy** — Transparent HTTP/HTTPS interception with on-the-fly certificate generation and LRU cert caching
- **Intercept Mode** — Pause requests for manual inspection, editing, forwarding, or dropping before they reach the server
- **Request History** — Browse, filter, and search captured traffic with syntax-highlighted bodies, JSON pretty-printing, and hex view for binary data
- **Repeater** — Edit and replay requests with side-by-side diff view comparing original and modified requests
- **Macros / Sequences** — Build ordered sequences of requests from history, then execute them step-by-step with per-step status tracking
- **Match & Replace Rules** — Modify requests, responses, or both automatically using regex or literal patterns across URL, headers, body, or all scopes; import and export rule sets as JSON
- **WebSocket Support** — Intercept, relay, and display WebSocket text and binary frames with direction and timestamp tracking
- **Scope Filtering** — Limit capture to specific hosts or wildcard domain patterns (e.g. `*.example.com`)
- **Passive Scanning** — Flag common security issues: missing HSTS/CSP/X-Frame-Options/X-Content-Type-Options headers, server/X-Powered-By information disclosure, insecure cookie flags (Secure, HttpOnly, SameSite), 5xx errors, and stack trace detection (Java, Python, .NET, Go)
- **Session Persistence** — Save (`Ctrl+S`) and load sessions to pick up where you left off; auto-generated timestamped session names
- **Import/Export** — Import HAR files; export as curl commands, raw HTTP, or HAR (HTTP Archive 1.2)
- **Encoding Tools** — Built-in URL, Base64, and Hex encode/decode utilities with real-time output and clipboard copy
- **Editor Modes** — Choose between a standard editor and Vim-style keybindings (with normal/insert modes, motions, and operators); toggle with `F2` or set via config/CLI
- **Multi-Instance Support** — Run multiple Crowbar instances simultaneously; automatic port selection finds the next available port if the default is occupied
- **Runtime Reconfiguration** — Change the proxy bind address and scope patterns without restarting

## Installation

Requires a Rust toolchain (1.85+, edition 2024).

```sh
cargo build --release
```

The binary is at `target/release/crowbar`.

## Usage

```sh
# Start the proxy (default: 127.0.0.1:8080)
crowbar

# Custom bind address
crowbar --bind 0.0.0.0:9090

# Start with intercept enabled
crowbar --intercept

# Start with Vim editor mode
crowbar --editor-mode vim

# Limit to specific hosts
crowbar --scope '*.example.com' --scope 'api.internal.dev'

# Load a previous session
crowbar --load ~/.crowbar/sessions/my-session.json
```

### CA Certificate

Crowbar generates a CA certificate on first run and stores it at `~/.crowbar/ca.pem`. Install it in your browser or system trust store to avoid TLS warnings.

```sh
# Export to a file
crowbar ca-export /path/to/crowbar-ca.pem

# Print to stdout
crowbar ca-export
```

You can also export the CA certificate from the Proxy tab in the TUI.

### Importing Data

```sh
# Import a HAR file
crowbar import recording.har --name my-session
```

### Rules Management

```sh
# Export a rules template to the default directory (~/.crowbar/rules/)
crowbar rules-export

# Export to a specific file
crowbar rules-export /path/to/rules.json

# Validate a rules file
crowbar rules-validate /path/to/rules.json
```

Rules can also be imported and exported from the Rules tab in the TUI.

### Configuration File

Optional config at `~/.crowbar/config.toml`:

```toml
bind = "127.0.0.1:8080"
intercept = false
scope = ["*.example.com"]
editor_mode = "default"  # or "vim"
```

CLI flags override config file values.

## TUI Tabs

The interface is organized into five tabs, switchable with `Tab`/`Shift+Tab` or number keys `1`–`5`. If the default port is in use, Crowbar automatically tries the next available port (up to 25 consecutive ports from the base).

1. **Proxy** — Live intercept queue, toggle intercept on/off, forward/drop/edit queued requests, change bind address, edit scope patterns, export CA certificate
2. **History** — Table of all captured requests (method, host, path, status, size, time) with filter bar, detail view showing request/response headers and bodies, security findings, and WebSocket messages
3. **Repeater** — Load a request from history, edit it freely, send it, and view the response; toggle a diff view to compare changes; manage macro sequences
4. **Rules** — Create, edit, enable/disable, and delete match & replace rules with configurable target (request/response/both), scope (URL/headers/body/all), and regex support
5. **Tools** — Cycle through encoding utilities (URL, Base64, Hex encode/decode) with a live input/output editor and clipboard copy

## Keyboard Shortcuts

### Global

| Key | Action |
|-----|--------|
| `Tab` / `Shift+Tab` | Switch tabs |
| `1`–`5` | Jump to tab |
| `?` | Show help overlay |
| `F2` | Toggle editor mode (Default / Vim) |
| `Ctrl+S` | Save session |
| `Ctrl+C` / `q` | Quit |

### Proxy Tab

| Key | Action |
|-----|--------|
| `i` | Toggle intercept on/off |
| `f` | Forward intercepted request |
| `d` | Drop intercepted request |
| `e` | Edit intercepted request |
| `b` | Change bind address |
| `s` | Edit scope patterns |
| `C` | Export CA certificate |
| `j` / `k` | Scroll request body |

### History Tab

| Key | Action |
|-----|--------|
| `j` / `k` / `↑` / `↓` | Navigate list / scroll detail |
| `g` / `G` | Jump to first / last |
| `/` | Filter by host, path, method, or status |
| `Enter` | Toggle detail view |
| `r` | Send to repeater |
| `m` | Add to macro sequence |
| `c` | Export as curl command |
| `w` | Export as raw HTTP |
| `h` | Export all as HAR |

### Repeater Tab

| Key | Action |
|-----|--------|
| `Ctrl+Enter` | Send request |
| `e` | Edit request |
| `d` | Toggle diff view |
| `M` | Toggle macro view |
| `j` / `k` | Scroll request |
| `J` / `K` | Scroll response |
| `x` | Remove macro step |
| `X` | Clear all macro steps |

### Rules Tab

| Key | Action |
|-----|--------|
| `a` | Add rule |
| `x` | Delete rule |
| `Enter` | Toggle enabled/disabled |
| `n` / `p` / `e` | Edit name / pattern / replacement |
| `t` / `s` | Cycle target / scope |
| `R` | Toggle regex mode |
| `E` | Export rules to file |
| `I` | Import rules from file |
| `j` / `k` | Navigate rules |

### Tools Tab

| Key | Action |
|-----|--------|
| `e` | Edit input |
| `←` / `h` | Previous tool |
| `→` / `l` | Next tool |
| `j` / `k` | Scroll output |
| `Ctrl+U` | Clear input |
| `Ctrl+Y` | Copy output to clipboard |

## Editor Modes

All text editors in Crowbar (intercept, repeater, tools, rules) share one of two modes, toggled at any time with `F2`:

### Default Mode

Standard text editing with arrow key navigation, Home/End, Ctrl+Home/Ctrl+End, Backspace/Delete, and Enter for new lines. Press `Esc` to exit the editor.

### Vim Mode

Enters **normal mode** by default. Press `i`, `a`, `I`, `A`, `o`, or `O` to switch to **insert mode**; press `Esc` to return to normal mode, and `q` in normal mode to exit the editor.

**Normal mode motions and operators:**

| Key | Action |
|-----|--------|
| `h` / `j` / `k` / `l` | Move left / down / up / right |
| `0` / `$` | Jump to line start / end |
| `^` | First non-whitespace character |
| `w` / `b` / `e` | Word forward / backward / end |
| `gg` / `G` | Go to beginning / end of text |
| `x` | Delete character at cursor |
| `D` | Delete to end of line |
| `dd` | Delete entire line |
| `dw` | Delete word |
| `d$` | Delete to end of line |
| `u` | Undo |
| `q` | Exit editor |

**Insert mode** supports the same editing keys as Default mode. Press `Esc` to return to normal mode.

Set the initial editor mode via CLI (`--editor-mode vim`) or config file (`editor_mode = "vim"`).

## File Locations

| Path | Purpose |
|------|---------|
| `~/.crowbar/ca.pem`, `~/.crowbar/ca.key` | Generated CA certificate and private key |
| `~/.crowbar/config.toml` | Optional configuration file |
| `~/.crowbar/sessions/` | Saved session files (JSON) |
| `~/.crowbar/rules/` | Exported/imported rule sets (JSON) |
| `~/.crowbar/exports/` | Exported data (HAR, curl, raw HTTP) |
| `~/.crowbar/crowbar.log` | Application log |

## License

MIT
