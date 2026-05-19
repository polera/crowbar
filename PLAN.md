# Crowbar Development Plan

## Phase 1: Core Usability

- [x] Request/response filtering and search in History tab
- [x] Scope rules — limit proxy to specific hosts/domains
- [x] Response body rendering — syntax highlight JSON, pretty-print HTML
- [x] CA cert export command (`crowbar ca-export`) so users can trust the root cert

## Phase 2: Data & Persistence

- [x] Persistent storage — save captured sessions to disk (SQLite or flat file)
- [x] Export requests/responses (HAR, curl commands, raw)
- [x] Import from HAR or saved sessions

## Phase 3: Protocol & Encoding

- [x] WebSocket interception and display
- [x] Encoding/decoding tools — URL encode/decode, base64, hex
- [x] Request body content-type awareness (form data, multipart, JSON)

## Phase 4: Advanced Features

- [x] Match-and-replace rules (auto-modify requests/responses)
- [x] Diff view between original and edited requests
- [x] Macro/sequence support — chain multiple requests
- [x] Passive vulnerability scanning (flag common issues in responses)

## Phase 5: gRPC Interception

- [x] gRPC-over-HTTP/2 detection and transparent proxying
- [x] Protobuf deserialization — decode request/response bodies using reflection or user-supplied `.proto` files
- [x] gRPC metadata display — surface headers, trailers, and status codes in the History tab
- [x] Streaming support — handle unary, server-streaming, client-streaming, and bidirectional calls
- [x] gRPC request editing and replay
