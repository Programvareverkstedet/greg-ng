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
