# Greg-ng

New implementation of https://github.com/Programvareverkstedet/grzegorz

## Test it out

```sh
# NixOS
nix run "git+https://git.pvv.ntnu.no/Grzegorz/greg-ng#" -- --mpv-socket-path /tmp/mpv.sock

# Other (after git clone and rust toolchain has been set up)
cargo run -- --mpv-socket-path /tmp/mpv.sock
```

See also https://git.pvv.ntnu.no/Grzegorz/grzegorz-clients for frontend alternatives

## Debugging

```sh
RUST_LOG=greg_ng=trace,mpvipc=trace cargo run -- --mpv-socket-path /tmp/mpv.sock
```

Custom api call in curl examples

LOL with input command. (utilizing ydotools)
```sh
curl -X POST -H "Content-Type: application/json" -d '{"keys": "42:1 38:1 38:0 24:1 24:0 38:1 38:0 42:0"}' http://localhost:8008/api/v2/sway/input/keys
```

Launching DEFAULT_BROWSER with url, in --kiosk mode
```sh
curl -X POST -H "Content-Type: application/json" -d '{"url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ"}' http://localhost:8008/api/v2/sway/browser/launch
```

