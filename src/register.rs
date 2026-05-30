use std::num::NonZeroU32;
use thiserror::Error;

#[cfg(all(unix, not(target_os = "macos")))]
pub use crate::linux::ServiceRegistration;
#[cfg(target_os = "macos")]
pub use crate::macos::ServiceRegistration;
#[cfg(target_os = "windows")]
pub use crate::windows::ServiceRegistration;

/// Builder for DNS-SD service registrations.
///
/// See [RFC 6763](https://datatracker.ietf.org/doc/html/rfc6763) for details on the semantics of the various parameters.
#[derive(Debug, Clone)]
pub struct ServiceRegistrationBuilder {
    pub(crate) service_type: String,
    pub(crate) port: u16,
    pub(crate) name: Option<String>,
    pub(crate) host: Option<String>,
    pub(crate) domain: Option<String>,
    pub(crate) interface_index: Option<NonZeroU32>,
    pub(crate) txt_record: Vec<(String, TxtRecordValue)>,
}

impl ServiceRegistrationBuilder {
    /// Initializes a new service registration builder with the given service type and port.
    ///
    /// - `service_type`:
    ///   The service type followed by the protocol, separated by a dot
    ///   (e.g. `_ftp._tcp`). The service type must be an underscore, followed
    ///   by 1-15 characters, which may be letters, digits, or hyphens.
    ///   The transport protocol must be `_tcp` or `_udp`. New service types
    ///   should be registered at <http://www.dns-sd.org/ServiceTypes.html>.
    ///   
    /// - `port`: The UDP/TCP port on which the service accepts connections.
    ///
    ///   Pass 0 for a "placeholder" service (i.e. a service that will not be discovered
    ///   by browsing, but will cause a name conflict if another client tries to
    ///   register that same name). Most clients will not use placeholder services.
    pub fn new(service_type: impl AsRef<str>, port: u16) -> Self {
        Self {
            service_type: service_type.as_ref().to_string(),
            port,
            name: None,
            host: None,
            domain: None,
            interface_index: None,
            txt_record: Vec::new(),
        }
    }

    /// Specifies the service name to be registered.
    ///
    /// Most applications will not specify a name, in which case the computer
    /// name is used. If a name is specified, it must be 1-63 bytes of UTF-8 text.
    pub fn name(&mut self, name: impl AsRef<str>) -> &mut Self {
        self.name = Some(name.as_ref().to_string());
        self
    }

    fn add_txt_record_key_value(&mut self, key: String, value: TxtRecordValue) {
        self.txt_record.retain_mut(|(k, _v)| *k != key);
        self.txt_record.push((key, value));
    }

    /// Adds a TXT record key to the service registration, with an empty value.
    pub fn add_txt_record_key_empty(&mut self, key: impl AsRef<str>) -> &mut Self {
        let key = key.as_ref().to_string();
        self.add_txt_record_key_value(key, TxtRecordValue::KeyOnly);
        self
    }

    /// Adds a TXT record key/value pair to the service registration.
    ///
    /// _Windows limitation:_ if `value` is an empty string, it will be treated as a key-only pair.
    pub fn add_txt_record_key_string(
        &mut self,
        key: impl AsRef<str>,
        value: impl AsRef<str>,
    ) -> &mut Self {
        let key = key.as_ref().to_string();
        let value = value.as_ref().to_string();
        self.add_txt_record_key_value(key, TxtRecordValue::String(value));
        self
    }

    /// Adds a TXT record key/value pair to the service registration, with a binary value.
    #[cfg(not(target_os = "windows"))] // Windows does not support binary TXT record values
    pub fn add_txt_record_key_binary(
        &mut self,
        key: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> &mut Self {
        let key = key.as_ref().to_string();
        let value = value.as_ref().to_vec();
        self.add_txt_record_key_value(key, TxtRecordValue::Binary(value));
        self
    }

    /// Specifies the interface on which to register the service
    /// (the index for a given interface is determined via the `if_nametoindex()`
    /// family of calls.)
    ///
    /// Most applications will not specify an interface, instead automatically
    /// registering on all available interfaces.
    pub fn interface_index(&mut self, index: NonZeroU32) -> &mut Self {
        self.interface_index = Some(index);
        self
    }

    /// Set the SRV target host name.
    ///
    /// Most applications will not specify a host, instead automatically using the machine's default
    /// host name(s).
    ///
    /// Note that specifying a host does NOT create an address record for that host.
    pub fn host(&mut self, host: impl AsRef<str>) -> &mut Self {
        self.host = Some(host.as_ref().to_string());
        self
    }

    /// Set the domain on which to advertise the service.
    ///
    /// Most applications will not specify a domain, instead automatically
    /// registering in the default domain(s).
    pub fn domain(&mut self, domain: impl AsRef<str>) -> &mut Self {
        self.domain = Some(domain.as_ref().to_string());
        self
    }

    /// Registers the service with the system, making it discoverable by remote clients.
    pub async fn register(&self) -> Result<ServiceRegistration, ServiceRegistrationError> {
        validate_txt_records(&self.txt_record)?;
        ServiceRegistration::new(
            &self.service_type,
            self.port,
            &self.name,
            &self.host,
            &self.domain,
            self.interface_index,
            &self.txt_record,
        )
        .await
    }
}

/// Error type for service registration failures.
#[derive(Error, Debug)]
pub enum ServiceRegistrationError {
    /// A TXT record key is invalid (empty, contains '=', or has non-ASCII characters).
    #[error("invalid TXT record key {0:?}: {1}")]
    InvalidTxtRecordKey(String, String),

    /// A TXT record value is too large (key + value exceeds 255 bytes).
    #[error("invalid TXT record value for key {0:?}: {1}")]
    InvalidTxtRecordValue(String, String),

    /// A string parameter contains an interior NUL byte.
    #[error("parameter {0:?} contains interior nul byte at position {1}")]
    ParameterContainsInteriorNulByte(String, usize),

    /// The interface index is not valid.
    #[error("interface index {0} is invalid")]
    InvalidInterfaceIndex(u32),

    /// The hostname could not be determined automatically.
    #[error("hostname not set and could not be determined automatically: {0}")]
    HostnameUnavailable(String),

    /// DNS-SD not available on system (Linux only - either D-Bus or Avahi unavailable).
    #[error("DNS-SD not available on system: {0}")]
    DnsSdUnavailable(String),

    /// The native DNS-SD API returned an error.
    #[error("DNS-SD registration returned an error: {0}")]
    RegistrationError(String),

    /// The registration failed.
    #[error("registration failed: {0}")]
    RegistrationFailed(String),

    /// A service name conflict was detected.
    #[error("service name conflict")]
    NameConflict,
}

#[derive(Debug, Clone)]
pub(crate) enum TxtRecordValue {
    KeyOnly,
    String(String),
    #[cfg(not(target_os = "windows"))] // Windows does not support binary TXT record values
    Binary(Vec<u8>),
}

/// Validates that all TXT record key/value pairs conform to DNS-SD rules.
pub(crate) fn validate_txt_records(
    records: &[(String, TxtRecordValue)],
) -> Result<(), ServiceRegistrationError> {
    for (key, value) in records {
        if key.is_empty() {
            return Err(ServiceRegistrationError::InvalidTxtRecordKey(
                key.clone(),
                "key must not be empty".into(),
            ));
        }
        if key.contains('=') {
            return Err(ServiceRegistrationError::InvalidTxtRecordKey(
                key.clone(),
                "key must not contain '='".into(),
            ));
        }
        // The characters of "Key" MUST be printable US-ASCII values
        // (0x20-0x7E) [RFC 20], excluding '=' (0x3D).
        if key.chars().any(|c| !(' '..='~').contains(&c)) {
            return Err(ServiceRegistrationError::InvalidTxtRecordKey(
                key.clone(),
                "key must contain only printable US-ASCII characters (0x20-0x7E)".into(),
            ));
        }
        let value_len = match value {
            TxtRecordValue::KeyOnly => 0,
            TxtRecordValue::String(s) => 1 + s.len(),
            #[cfg(not(target_os = "windows"))]
            TxtRecordValue::Binary(b) => 1 + b.len(),
        };
        if key.len() + value_len > 255 {
            return Err(ServiceRegistrationError::InvalidTxtRecordValue(
                key.clone(),
                "key + value must not exceed 255 bytes".into(),
            ));
        }
    }
    Ok(())
}
