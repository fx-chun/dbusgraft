This connects two dbus sockets together, forwarding messages from one to the other (i.e. acting as a server on one, client on the other). This is unlike xdg-dbus-proxy which connects to one socket and _creates_ another socket.

I use this in conjunction with `xdg-dbus-proxy` for containers, where I use the `proxy` to make a notification-only socket I mount into containers, then `dbusgraft` to reroute notification messages from the container bus to the proxied bus.

# Help

```
dbusgraft -h                                                                                         
Usage: /mnt/home-dev/.cargo_target/debug/dbusgraft ...FLAGS

    Connects to 2 dbus sockets and registers as a server for a set of services on the downstream, and acts as a client on the upstream.  Any requests it receives as a server it'll forward to the cooresponding server on the upstream side.

    --upstream <STRING>            The connection with the desired servers. As you'd see in `DBUS_SESSION_BUS_ADDRESS`, ex: `unix:path=/run/user/1910/bus`
    --downstream <STRING>          The connection with no servers, where this will pose as a server. Ex: `unix:path=/run/user/1910/bus`
    --register <STRING>[ ...]      The names to act as a server for/forward. Ex: `org.freedesktop.Notifications`

```

# Installation

1. Clone the repo
2. Do `cargo build`
3. The binary will be in `target/debug/dbusgraft`