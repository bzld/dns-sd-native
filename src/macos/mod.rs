mod ffi;

use ffi::*;
use log::trace;
use std::{ffi::CString, num::NonZeroU32, sync::Mutex};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::{ServiceRegistrationError, TxtRecordValue};

/// Reference to a registered service instance.
///
/// The service will be automatically unregistered when this value is dropped.
pub struct ServiceRegistration {
    reference: Option<(DNSServiceRef, Box<CallbackContext>)>,
}

/// Heap-allocated context passed to `DNSServiceRegister` as the callback context.
struct CallbackContext {
    tx: Mutex<Option<oneshot::Sender<DNSServiceErrorType>>>,
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
        let service_type = CString::new(service_type.as_bytes()).map_err(|e| {
            ServiceRegistrationError::ParameterContainsInteriorNulByte(
                service_type.clone(),
                e.nul_position(),
            )
        })?;

        let name: Option<CString> = name
            .as_ref()
            .map(|name| {
                CString::new(name.as_bytes()).map_err(|e| {
                    ServiceRegistrationError::ParameterContainsInteriorNulByte(
                        name.clone(),
                        e.nul_position(),
                    )
                })
            })
            .transpose()?;

        let domain: Option<CString> = domain
            .as_ref()
            .map(|domain| {
                CString::new(domain.as_bytes()).map_err(|e| {
                    ServiceRegistrationError::ParameterContainsInteriorNulByte(
                        domain.clone(),
                        e.nul_position(),
                    )
                })
            })
            .transpose()?;

        let host: Option<CString> = host
            .as_ref()
            .map(|host| {
                CString::new(host.as_bytes()).map_err(|e| {
                    ServiceRegistrationError::ParameterContainsInteriorNulByte(
                        host.clone(),
                        e.nul_position(),
                    )
                })
            })
            .transpose()?;

        let interface = interface_index.map(|i| i.get()).unwrap_or(0); // 0 means all interfaces

        let mut txt_record = Vec::new();
        for (key, value) in txt_record_values {
            match value {
                TxtRecordValue::KeyOnly => {
                    let key_bytes = key.as_bytes();
                    txt_record.push(key_bytes.len() as u8);
                    txt_record.extend_from_slice(key_bytes);
                }
                TxtRecordValue::String(s) => {
                    let key_bytes = key.as_bytes();
                    let value_bytes = s.as_bytes();
                    txt_record.push((key_bytes.len() + 1 + value_bytes.len()) as u8);
                    txt_record.extend_from_slice(key_bytes);
                    txt_record.push(b'=');
                    txt_record.extend_from_slice(value_bytes);
                }
                TxtRecordValue::Binary(b) => {
                    let key_bytes = key.as_bytes();
                    txt_record.push((key_bytes.len() + 1 + b.len()) as u8);
                    txt_record.extend_from_slice(key_bytes);
                    txt_record.push(b'=');
                    txt_record.extend_from_slice(b);
                }
            }
        }
        if txt_record_values.is_empty() {
            // DNS-SD requires at least one TXT record, so add an empty one if none were provided
            txt_record.push(0);
        }

        trace!(
            "Registering service with type: {:?}, port: {}, name: {:?}, host: {:?}, domain: {:?}, interface_index: {:?}, txt_record: {:?}",
            service_type, port, name, host, domain, interface_index, txt_record
        );

        // we use the KISS method of calling DNSServiceProcessResult synchronously after DNSServiceRegister,
        // which is ok because we're doing it in a blocking task, so it won't block the async runtime.
        let registration = tokio::task::spawn_blocking(move || {
            let mut service_ref = DNSServiceRef::default();
            let (tx, mut rx) = oneshot::channel();
            let ctx = Box::new(CallbackContext {
                tx: Mutex::new(Some(tx)),
            });
            let ctx_ptr: *const CallbackContext = &*ctx;
            let error = unsafe {
                DNSServiceRegister(
                    &mut service_ref,
                    0,
                    interface,
                    name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr()),
                    service_type.as_ptr(),
                    domain
                        .as_ref()
                        .map_or(std::ptr::null(), |domain| domain.as_ptr()),
                    host.as_ref().map_or(std::ptr::null(), |host| host.as_ptr()),
                    port.to_be(),
                    txt_record.len() as u16,
                    txt_record.as_ptr() as *const std::ffi::c_void,
                    Some(callback),
                    ctx_ptr as *mut std::ffi::c_void,
                )
            };
            trace!("DNSServiceRegister returned: {error}");

            if error != error::NO_ERROR {
                if error == error::NAME_CONFLICT {
                    return Err(ServiceRegistrationError::NameConflict);
                }
                return Err(ServiceRegistrationError::RegistrationError(format!(
                    "DNSServiceRegister failed with error code: {error}"
                )));
            }

            let process_error = unsafe { DNSServiceProcessResult(service_ref.0) };
            trace!("DNSServiceProcessResult returned: {process_error}");

            if process_error != error::NO_ERROR {
                unsafe {
                    DNSServiceRefDeallocate(service_ref);
                }
                return Err(ServiceRegistrationError::RegistrationError(format!(
                    "DNSServiceProcessResult failed with error code: {process_error}"
                )));
            }

            let callback_status = match rx.try_recv() {
                Ok(status) => status,
                Err(_) => {
                    unsafe {
                        DNSServiceRefDeallocate(service_ref);
                    }
                    return Err(ServiceRegistrationError::RegistrationError(
                        "DNSServiceRegister callback did not fire".into(),
                    ));
                }
            };
            trace!("Callback status: {callback_status}");
            if callback_status != error::NO_ERROR {
                unsafe {
                    DNSServiceRefDeallocate(service_ref);
                }
                if callback_status == error::NAME_CONFLICT {
                    return Err(ServiceRegistrationError::NameConflict);
                }
                return Err(ServiceRegistrationError::RegistrationError(format!(
                    "DNSServiceRegister callback failed with error code: {callback_status}"
                )));
            }
            Ok(ServiceRegistration {
                reference: Some((service_ref, ctx)),
            })
        })
        .await
        .map_err(|err| {
            ServiceRegistrationError::RegistrationError(format!(
                "service registration task failed: {err}"
            ))
        })??;

        Ok(registration)
    }

    /// Unregisters the service, notifying remote clients that the service is no longer available.
    ///
    /// Use this method instead of dropping the `ServiceRegistration` if you want to be notified of
    /// any errors that occur during unregistration.
    pub async fn unregister(mut self) -> Result<(), String> {
        if let Some(join_handle) = self.deallocate() {
            join_handle
                .await
                .map_err(|err| format!("service unregistration panicked: {err:?}"))?;
        }
        Ok(())
    }

    fn deallocate(&mut self) -> Option<JoinHandle<()>> {
        self.reference.take().map(|(service_ref, ctx)| {
            tokio::task::spawn_blocking(move || unsafe {
                DNSServiceRefDeallocate(service_ref);
                drop(ctx);
            })
        })
    }
}

impl Drop for ServiceRegistration {
    fn drop(&mut self) {
        self.deallocate();
    }
}

unsafe extern "C" fn callback(
    _service_ref: DNSServiceRef,
    _flags: DNSServiceFlags,
    error_code: DNSServiceErrorType,
    _name: *const ::std::os::raw::c_char,
    _regtype: *const ::std::os::raw::c_char,
    _domain: *const ::std::os::raw::c_char,
    context: *mut ::std::os::raw::c_void,
) {
    trace!("Service registration callback with error_code: {error_code}");

    // SAFETY: `context` points to a heap-allocated `CallbackContext` owned by
    // the originating `ServiceRegistration`. That allocation is only freed
    // after `DNSServiceRefDeallocate` returns, after which no more callbacks
    // will be made.
    let ctx = unsafe { &*(context as *const CallbackContext) };
    if let Ok(mut lock) = ctx.tx.lock() {
        if let Some(tx) = lock.take() {
            let _ = tx.send(error_code);
        }
    }
}
