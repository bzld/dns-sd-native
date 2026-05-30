# `dns-sd-native`

[![Crates.io](https://img.shields.io/crates/v/dns-sd-native.svg)](https://crates.io/crates/dns-sd-native)
[![Documentation](https://docs.rs/dns-sd-native/badge.svg)](https://docs.rs/dns-sd-native)
[![License: MIT](https://img.shields.io/crates/l/dns-sd-native.svg)](LICENSE)

Access the operating system's built-in
[DNS-SD](https://en.wikipedia.org/wiki/Zero-configuration_networking#DNS-based_service_discovery) /
[mDNS](https://en.wikipedia.org/wiki/Multicast_DNS) stack for service registration.

This crate provides a cross-platform async API (using [Tokio](https://tokio.rs)) to register
DNS-SD services via native OS facilities — no bundled mDNS implementation, no extra system
dependencies to install.

> **Note:** This crate currently supports _registration_ of services only, not browsing/discovery.

## Platform Support

| Platform | Backend | Minimum Version |
|----------|---------|-----------------|
| macOS | native [DNS-SD framework](https://developer.apple.com/documentation/dnssd) | macOS 10.12 |
| Windows | native [Win32 DNS-SD API](https://learn.microsoft.com/en-us/uwp/api/windows.networking.servicediscovery.dnssd?view=winrt-28000) | Windows 10 |
| Linux / BSD | [Avahi](https://avahi.org) via D-Bus | Avahi daemon running |

- **macOS** and **Windows** link against system libraries that are always present on supported OS versions.
- **Linux/FreeBSD** communicate with Avahi over D-Bus — there is no binary dependency on `libavahi` or the Bonjour compatibility layer. Binaries will run on systems without Avahi installed but return an error when attempting to register.

## Why Use the OS Stack?

There exist various crates implementing DNS-SD/mDNS in pure Rust. Compared to these, using the operating system's DNS-SD stack has the following benefits:

- **Battle-tested**: OS stacks have been tested widely over many years (sometimes decades) to handle OS-dependent edge cases (sleep/wake, interface changes) properly 
- **Automatic cleanup**: registered services are automatically deregistered when the process exits, even on crash or `SIGKILL`.
- **Shared cache**: all applications on the system share a single DNS-SD/mDNS responder & cache, reducing network traffic and keeping answers consistent.
- **Smaller binary**: no embedded mDNS responder; just thin FFI/D-Bus bindings.

There also exist various crates implementing a wrapper on top of the Apple DNS-SD API, which is natively available on macOS, but only on
Windows if users install Bonjour for Windows, and on Linux if the `libavahi-compat-libdnssd` library is installed. Compared to these, this
crate requires no additional dependencies to install for end users.

## Example

Register a service of type `_http._tcp` on port 8080:

```rust
use dns_sd_native::ServiceRegistrationBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = ServiceRegistrationBuilder::new("_http._tcp", 8080)
        .name("My Web Server")
        .register()
        .await?;

    // Service is now discoverable on the local network.
    // Unregister explicitly (or just drop the value):
    service.unregister().await?;
    Ok(())
}
```

See the [`examples/`](examples/) directory for a full CLI tool that registers arbitrary services.

## License

[MIT](LICENSE)
