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

- **MITM Proxy** — Transparent HTTP/HTTPS interception with on-the-fly certificate generation
- **Intercept Mode** — Pause requests for manual inspection and editing before forwarding
- **Request History** — Browse, filter, and search captured traffic with syntax-highlighted bodies
- **Repeater** — Edit and replay requests manually
- **Match & Replace Rules** — Modify requests/responses automatically using regex or literal patterns
- **WebSocket Support** — Intercept and display WebSocket frames
- **Scope Filtering** — Limit capture to specific hosts or domain patterns
- **Passive Scanning** — Flag common security issues (missing headers, cookie flags, info disclosure)
- **Session Persistence** — Save and load sessions to pick up where you left off
- **Import/Export** — HAR files, curl commands, and raw HTTP
- **Encoding Tools** — Built-in URL, Base64, and Hex encode/decode utilities

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

### Configuration File

Optional config at `~/.crowbar/config.toml`:

```toml
bind = "127.0.0.1:8080"
intercept = false
scope = ["*.example.com"]
```

CLI flags override config file values.

## TUI Navigation

| Key | Action |
|-----|--------|
| `Tab` / `Shift+Tab` | Switch tabs |
| `↑` / `↓` | Navigate lists |
| `Enter` | Select / confirm |
| `/` | Filter / search |
| `i` | Toggle intercept mode |
| `f` | Forward intercepted request |
| `d` | Drop intercepted request |
| `q` | Quit |

## License

MIT
