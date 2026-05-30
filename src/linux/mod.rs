use zbus::Connection;

mod dbus;
use dbus::*;
use futures_util::stream::StreamExt;

use log::{error, trace, warn};
use std::num::NonZeroU32;

use crate::{ServiceRegistrationError, TxtRecordValue};

const AVAHI_IF_UNSPEC: i32 = -1;
const AVAHI_PROTO_UNSPEC: i32 = -1;

/// Reference to a registered service instance.
///
/// The service will be automatically unregistered when this value is dropped.
pub struct ServiceRegistration {
    entry_group: Option<EntryGroupProxy<'static>>,
}

impl ServiceRegistration {
    pub(crate) async fn new(
        service_type: &String,
        port: u16,
        name: &Option<String>,
        host: &Option<String>,
        domain: &Option<String>,
        interface_index: Option<NonZeroU32>,
        txt_record_values: &[(String, TxtRecordValue)],
    ) -> Result<ServiceRegistration, ServiceRegistrationError> {
        let conn = Connection::system().await.map_err(|err| {
            ServiceRegistrationError::DnsSdUnavailable(format!(
                "failed to connect to system D-Bus: {err}"
            ))
        })?;

        let manager = AvahiProxy::new(&conn).await.map_err(|err| {
            ServiceRegistrationError::DnsSdUnavailable(format!(
                "failed to connect to Avahi via D-Bus: {err}"
            ))
        })?;
        let entry_group = manager.entry_group_new().await.map_err(|err| {
            ServiceRegistrationError::RegistrationError(format!(
                "failed to create Avahi entry group: {err}"
            ))
        })?;

        let protocol = AVAHI_PROTO_UNSPEC;
        let flags = 0;

        let interface = match interface_index {
            Some(i) => {
                let idx = i.get();
                if idx > i32::MAX as u32 {
                    return Err(ServiceRegistrationError::InvalidInterfaceIndex(idx));
                }
                idx as i32
            }
            None => AVAHI_IF_UNSPEC,
        };
        let domain = domain.as_deref().unwrap_or("");
        let host = host.as_deref().unwrap_or("");
        let name = if let Some(name) = name {
            name.as_str()
        } else {
            &manager
                .get_host_name()
                .await
                .map_err(|err| ServiceRegistrationError::HostnameUnavailable(err.to_string()))?
        };
        let txt: Vec<Vec<u8>> = txt_record_values
            .iter()
            .map(|(key, value)| {
                let mut record = key.clone().into_bytes();
                match value {
                    TxtRecordValue::KeyOnly => {}
                    TxtRecordValue::String(s) => {
                        record.push(b'=');
                        record.extend_from_slice(s.as_bytes());
                    }
                    TxtRecordValue::Binary(b) => {
                        record.push(b'=');
                        record.extend_from_slice(b);
                    }
                }
                record
            })
            .collect();

        let txt_refs: Vec<&[u8]> = txt.iter().map(|v| v.as_slice()).collect();

        trace!(
            "registering service with Avahi: interface={:?} protocol={:?} flags={:?} name={:?} type={:?} domain={:?} host={:?} port={:?} txt={:?}",
            interface, protocol, flags, name, service_type, domain, host, port, txt
        );
        entry_group
            .add_service(
                interface,
                protocol,
                flags,
                name,
                service_type,
                domain,
                host,
                port,
                &txt_refs,
            )
            .await
            .map_err(|err| {
                ServiceRegistrationError::RegistrationError(format!(
                    "Avahi add_service failed: {err}"
                ))
            })?;
        // TODO: return ServiceRegistrationError::NameConflict if Avahi returns ErrorName == "org.freedesktop.Avahi.CollisionError"

        entry_group.commit().await.map_err(|err| {
            ServiceRegistrationError::RegistrationError(format!(
                "Avahi entry group commit failed: {err}"
            ))
        })?;

        match entry_group.get_state().await {
            Ok(AVAHI_ENTRY_GROUP_ESTABLISHED) => {
                trace!("service registration state: established");
            }
            Ok(AVAHI_ENTRY_GROUP_COLLISION) => {
                return Err(ServiceRegistrationError::NameConflict);
            }
            Ok(AVAHI_ENTRY_GROUP_FAILURE) => {
                return Err(ServiceRegistrationError::RegistrationFailed(
                    "entry group entered failure state".into(),
                ));
            }
            Err(err) => {
                return Err(ServiceRegistrationError::RegistrationError(format!(
                    "Avahi entry group get_state failed: {err}"
                )));
            }
            Ok(state) => {
                if state != AVAHI_ENTRY_GROUP_REGISTERING {
                    warn!("service registration state: unknown state: {state}");
                }
                let mut state_stream = entry_group
                    .receive_state_changed()
                    .await
                    .map_err(|err| ServiceRegistrationError::RegistrationError(err.to_string()))?;
                while let Some(msg) = state_stream.next().await {
                    let args = msg.args().map_err(|err| {
                        ServiceRegistrationError::RegistrationError(err.to_string())
                    })?;
                    trace!(
                        "state changed: state={:?} error={:?}",
                        args.state, args.error
                    );
                    match args.state {
                        AVAHI_ENTRY_GROUP_REGISTERING => continue,
                        AVAHI_ENTRY_GROUP_ESTABLISHED => {
                            trace!("service registration state: established");
                        }
                        AVAHI_ENTRY_GROUP_COLLISION => {
                            return Err(ServiceRegistrationError::NameConflict);
                        }
                        AVAHI_ENTRY_GROUP_FAILURE => {
                            return Err(ServiceRegistrationError::RegistrationFailed(format!(
                                "entry group failure: {}",
                                args.error
                            )));
                        }
                        _ => {
                            warn!("service registration state: unknown state: {args:?}");
                        }
                    }
                    break;
                }
            }
        }

        Ok(Self {
            entry_group: Some(entry_group),
        })
    }

    /// Unregisters the service, notifying remote clients that the service is no longer available.
    ///
    /// Use this method instead of dropping the `ServiceRegistration` if you want to be notified of
    /// any errors that occur during unregistration.
    pub async fn unregister(mut self) -> Result<(), String> {
        if let Some(entry_group) = self.entry_group.take() {
            entry_group
                .reset()
                .await
                .map_err(|err| format!("failed to unregister service: {err:?}"))
        } else {
            Ok(())
        }
    }
}

impl Drop for ServiceRegistration {
    fn drop(&mut self) {
        if let Some(entry_group) = self.entry_group.take() {
            tokio::spawn(async move {
                if let Err(err) = entry_group.reset().await {
                    error!("failed to unregister service: {err}");
                }
            });
        }
    }
}
