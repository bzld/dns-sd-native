#![warn(missing_docs)]
#![warn(unused_extern_crates, unused_qualifications)]

//! Access the operating system's built-in
//! [DNS-SD](https://en.wikipedia.org/wiki/Zero-configuration_networking#DNS-based_service_discovery) /
//! [mDNS](https://en.wikipedia.org/wiki/Multicast_DNS) stack for service registration.
//!
//! This crate provides a cross-platform async API (using [Tokio](https://tokio.rs)) for
//! registering DNS-SD services via the native OS facilities:
//!
//! - **macOS**: native [DNS-SD framework](https://developer.apple.com/documentation/dnssd) (available since macOS 10.12)
//! - **Windows**: native [Win32 DNS-SD API](https://learn.microsoft.com/en-us/uwp/api/windows.networking.servicediscovery.dnssd?view=winrt-28000) (available since Windows 10)
//! - **Linux/FreeBSD**: [Avahi](https://avahi.org) via D-Bus (no binary dependency on libavahi)
//!
//! **Note:** This crate currently supports _registration_ of services only, not browsing/discovery.
//!
//! # Example
//!
//! ```no_run
//! use dns_sd_native::ServiceRegistrationBuilder;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let service = ServiceRegistrationBuilder::new("_http._tcp", 8080)
//!     .name("My Web Server")
//!     .register()
//!     .await?;
//!
//! // Service is now discoverable on the network.
//! // It will be automatically unregistered when dropped,
//! // or you can unregister explicitly:
//! service.unregister().await?;
//! # Ok(())
//! # }
//! ```

pub use self::register::*;

mod register;

#[cfg(all(unix, not(target_os = "macos")))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
