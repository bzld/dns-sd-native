use std::net::IpAddr;
use std::num::NonZeroU32;
use std::str::FromStr;

use clap::Parser;
use log::{error, info, trace};
use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};

use dns_sd_native::ServiceRegistrationBuilder;

/// Register a DNS-SD service and keep it alive until stopped.
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// Service type, e.g. `_ssh._tcp` or `_ntp._udp`.  See <http://www.dns-sd.org/ServiceTypes.html>.
    service_type: String,

    /// TCP/UDP port the service listens on (0 = placeholder)
    port: u16,

    /// Service instance name (defaults to the machine hostname)
    #[arg(short, long)]
    name: Option<String>,

    /// Domain to advertise on (defaults to all default domains)
    #[arg(short, long)]
    domain: Option<String>,

    /// SRV target hostname (defaults to the machine's hostname(s))
    #[arg(short = 'H', long)]
    host: Option<String>,

    /// Interface to register on: an IP address or interface name.
    /// Registers on all interfaces if omitted.
    #[arg(short, long, value_name = "IP_OR_NAME")]
    interface: Option<String>,

    /// Add a TXT record entry.  Use `KEY` for a key-only (empty) entry or
    /// `KEY=VALUE` for a string value.  May be repeated.
    #[arg(long = "txt", value_name = "KEY[=VALUE]")]
    txt: Vec<String>,

    /// Add a TXT record entry with a binary value encoded as hex, e.g.
    /// `bin=0102ff`.  May be repeated.
    #[cfg(not(windows))] // Windows implementation does not support binary TXT values
    #[arg(long = "txt-binary", value_name = "KEY=HEX")]
    txt_binary: Vec<String>,

    /// Enable trace-level logging (default is debug)
    #[arg(short, long)]
    verbose: bool,

    /// Exit after SECS seconds instead of waiting for Ctrl-C
    #[arg(short, long, value_name = "SECS")]
    wait: Option<u64>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Configure logging: debug by default, trace if --verbose.
    env_logger::Builder::new()
        .filter_level(if args.verbose {
            log::LevelFilter::Trace
        } else {
            log::LevelFilter::Debug
        })
        .parse_default_env() // still honour RUST_LOG if set
        .init();

    let mut builder = ServiceRegistrationBuilder::new(&args.service_type, args.port);

    if let Some(name) = &args.name {
        builder.name(name);
    }
    if let Some(domain) = &args.domain {
        builder.domain(domain);
    }
    if let Some(host) = &args.host {
        builder.host(host);
    }
    if let Some(iface) = &args.interface {
        builder.interface_index(resolve_interface(iface));
    }

    for entry in &args.txt {
        match parse_txt(entry) {
            (key, None) => {
                builder.add_txt_record_key_empty(key);
            }
            (key, Some(value)) => {
                builder.add_txt_record_key_string(key, value);
            }
        }
    }

    #[cfg(not(windows))] // Windows implementation does not support binary TXT values
    for entry in &args.txt_binary {
        let (key, bytes) = parse_txt_binary(entry);
        builder.add_txt_record_key_binary(key, bytes);
    }

    let service = builder
        .register()
        .await
        .expect("failed to register service");

    info!("registered service");

    match args.wait {
        Some(secs) => {
            info!("waiting {secs}s... (press Ctrl-C to exit early)");
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        }
        None => {
            info!("press Ctrl-C to unregister and exit");
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for Ctrl-C");
        }
    }

    info!("unregistering service...");
    if let Err(err) = service.unregister().await {
        error!("failed to unregister service: {err}");
    } else {
        info!("done");
    }
}

/// Resolve an interface specifier (IP address or interface name) to a
/// `NonZeroU32` interface index, or exit with an error.
fn resolve_interface(spec: &str) -> NonZeroU32 {
    let target_ip = IpAddr::from_str(spec).ok();

    let interfaces = match NetworkInterface::show() {
        Ok(ifaces) => ifaces,
        Err(e) => {
            error!("error: failed to enumerate network interfaces: {e}");
            std::process::exit(1);
        }
    };

    for iface in &interfaces {
        let matched = if let Some(ip) = target_ip {
            iface.addr.iter().any(|addr| match (addr, ip) {
                (Addr::V4(v4), IpAddr::V4(target)) => v4.ip == target,
                (Addr::V6(v6), IpAddr::V6(target)) => v6.ip == target,
                _ => false,
            })
        } else {
            iface.name == spec
        };

        if matched {
            match NonZeroU32::new(iface.index) {
                Some(idx) => {
                    trace!("resolved interface '{}' → index {}", spec, idx);
                    return idx;
                }
                None => {
                    error!(
                        "error: interface '{}' has index 0, which is invalid",
                        iface.name
                    );
                    std::process::exit(1);
                }
            }
        }
    }

    error!("error: no interface found matching '{spec}'");
    std::process::exit(1);
}

/// Parse a `--txt KEY` or `--txt KEY=VALUE` entry.
/// Returns `(key, Some(value))` for a string entry or `(key, None)` for key-only.
fn parse_txt(s: &str) -> (&str, Option<&str>) {
    match s.find('=') {
        Some(pos) => (&s[..pos], Some(&s[pos + 1..])),
        None => (s, None),
    }
}

/// Parse a `--txt-binary KEY=HEX` entry and hex-decode the value.
#[cfg(not(windows))] // Windows implementation does not support binary TXT values
fn parse_txt_binary(s: &str) -> (&str, Vec<u8>) {
    let pos = match s.find('=') {
        Some(p) => p,
        None => {
            error!("error: --txt-binary '{s}' must be in the form KEY=HEXBYTES");
            std::process::exit(1);
        }
    };
    let key = &s[..pos];
    let hex_str = &s[pos + 1..];

    let bytes = match hex::decode(hex_str) {
        Ok(b) => b,
        Err(e) => {
            error!("error: --txt-binary hex value for key '{key}': {e}");
            std::process::exit(1);
        }
    };

    (key, bytes)
}
