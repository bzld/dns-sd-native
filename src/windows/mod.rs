use widestring::U16CString;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::NetworkManagement::Dns;
use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
use windows::core::PCWSTR;
use windows::core::PWSTR;
use windows_strings::*;

use std::ffi::c_void;
use std::ptr::NonNull;

use log::{error, trace};
use std::num::NonZeroU32;
use tokio::sync::oneshot;

use crate::{ServiceRegistrationError, TxtRecordValue};

const DNS_QUERY_REQUEST_VERSION1: u32 = 1;
const DNS_REQUEST_PENDING: u32 = 9506;

/// RAII wrapper around a `DNS_SERVICE_INSTANCE` produced by `DnsServiceConstructInstance`.
/// Dropping it releases the instance via `DnsServiceFreeInstance`.
struct ServiceInstance(NonNull<Dns::DNS_SERVICE_INSTANCE>);

// SAFETY: This wrapper is the sole owner of the underlying `DNS_SERVICE_INSTANCE`
// heap allocation. The raw pointer is only ever used as an opaque handle passed
// to the DNS-SD API (register / deregister / free) and is never dereferenced in
// Rust, so the value is safe to move and share between threads.
unsafe impl Send for ServiceInstance {}
unsafe impl Sync for ServiceInstance {}

impl ServiceInstance {
    fn new(
        instance_name: &str,
        host_name: &str,
        port: u16,
        priority: u16,
        weight: u16,
        key_value_pairs: &[(U16CString, U16CString)],
    ) -> Result<Self, ServiceRegistrationError> {
        let instance_name = HSTRING::from(instance_name);
        let host_name = HSTRING::from(host_name);

        let key_ptrs: Vec<PCWSTR> = key_value_pairs
            .iter()
            .map(|(k, _)| PCWSTR(k.as_ptr()))
            .collect();
        let value_ptrs: Vec<PCWSTR> = key_value_pairs
            .iter()
            .map(|(_, v)| PCWSTR(v.as_ptr()))
            .collect();

        // SAFETY: `instance_name` / `host_name` are valid HSTRINGs and the
        // key/value pointer arrays (and their backing `key_value_pairs` vec)
        // outlive this call.
        let instance = unsafe {
            Dns::DnsServiceConstructInstance(
                &instance_name,
                &host_name,
                None,
                None,
                port,
                priority,
                weight,
                key_value_pairs.len().try_into().unwrap(),
                key_ptrs.as_ptr(),
                value_ptrs.as_ptr(),
            )
        };

        NonNull::new(instance).map(Self).ok_or_else(|| {
            ServiceRegistrationError::RegistrationFailed(
                "DnsServiceConstructInstance returned null".into(),
            )
        })
    }

    /// Returns the raw instance pointer for passing to the DNS-SD API.
    fn as_ptr(&self) -> *mut Dns::DNS_SERVICE_INSTANCE {
        self.0.as_ptr()
    }
}

impl Drop for ServiceInstance {
    fn drop(&mut self) {
        // SAFETY: the instance was produced by `DnsServiceConstructInstance` and,
        // as the sole owner, we free it exactly once here.
        unsafe {
            Dns::DnsServiceFreeInstance(self.0.as_ptr());
        }
    }
}

/// Context passed through the DNS-SD callback's `pQueryContext` parameter.
///
/// The context owns the [`ServiceInstance`] for the duration of an in-flight
/// register/deregister operation, keeping it alive until the OS invokes the
/// completion callback.
struct CallbackContext {
    /// Channel used to hand the instance and status back to a waiting caller.
    /// `None` for fire-and-forget operations (e.g. unregister on drop).
    status_tx: Option<oneshot::Sender<(u32, ServiceInstance)>>,
    /// The instance the in-flight operation refers to.
    instance: ServiceInstance,
}

/// Reference to a registered service instance. The service will be automatically unregistered when this value is dropped.
pub struct ServiceRegistration {
    instance: Option<ServiceInstance>,
    interface_index: Option<NonZeroU32>,
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
        let key_value_pairs = txt_record_values
            .iter()
            .map(|(key, value)| {
                let k = U16CString::from_str(key.as_str()).map_err(|err| {
                    ServiceRegistrationError::ParameterContainsInteriorNulByte(
                        key.clone(),
                        err.nul_position(),
                    )
                })?;
                let v = match value {
                    TxtRecordValue::KeyOnly => U16CString::new(),
                    TxtRecordValue::String(s) => {
                        U16CString::from_str(s.as_str()).map_err(|err| {
                            ServiceRegistrationError::ParameterContainsInteriorNulByte(
                                s.clone(),
                                err.nul_position(),
                            )
                        })?
                    }
                };
                Ok((k, v))
            })
            .collect::<Result<Vec<_>, ServiceRegistrationError>>()?;

        let domain_str = domain.as_deref().unwrap_or("local");
        let hostname = if let Some(host) = host {
            host
        } else {
            &get_hostname().map_err(ServiceRegistrationError::HostnameUnavailable)?
        };
        let instance_name = name.as_deref().unwrap_or(hostname);

        let priority = 0;
        let weight = 0;

        let instance = ServiceInstance::new(
            &format!("{instance_name}.{service_type}.{domain_str}"),
            &format!("{hostname}.{domain_str}"),
            port,
            priority,
            weight,
            &key_value_pairs,
        )?;

        let (tx, rx) = oneshot::channel::<(u32, ServiceInstance)>();

        register(instance, interface_index, Some(tx))?;

        let (status, instance) = rx.await.map_err(|_| {
            ServiceRegistrationError::RegistrationError(
                "registration callback channel closed unexpectedly".into(),
            )
        })?;

        if status != ERROR_SUCCESS.0 {
            return Err(ServiceRegistrationError::RegistrationError(format!(
                "registration failed with status: {status}"
            )));
        }

        Ok(ServiceRegistration {
            instance: Some(instance),
            interface_index,
        })
    }

    /// Unregisters the service, notifying remote clients that the service is no longer available.
    /// Use this method instead of dropping the `ServiceRegistration` if you want to be notified of
    /// any errors that occur during unregistration.
    pub async fn unregister(mut self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel::<(u32, ServiceInstance)>();
        self.deregister(Some(tx))?;
        let (status, _instance) = rx
            .await
            .map_err(|_| "deregistration callback channel closed unexpectedly".to_string())?;
        if status != ERROR_SUCCESS.0 {
            return Err(format!(
                "service deregistration failed with status: {status}"
            ));
        }
        Ok(())
    }

    fn deregister(
        &mut self,
        status_tx: Option<oneshot::Sender<(u32, ServiceInstance)>>,
    ) -> Result<(), String> {
        let Some(instance) = self.instance.take() else {
            return Ok(());
        };

        let request = build_request(instance, self.interface_index, status_tx);

        // SAFETY: `request` references the instance and context, both owned by the
        // `CallbackContext` behind `context_ptr`, which stays alive until the
        // completion callback reclaims it.
        let result = unsafe { Dns::DnsServiceDeRegister(&request, None) };
        trace!("DnsServiceDeRegister result: {result}");

        if result != DNS_REQUEST_PENDING {
            // The callback will not fire; reclaim the context (which frees the
            // instance on drop).
            // SAFETY: `context_ptr` came from `Box::into_raw` in `build_request`.
            let context_ptr = request.pQueryContext as *mut CallbackContext;
            unsafe {
                drop(Box::from_raw(context_ptr));
            }
            return Err(format!(
                "deregistration failed to start with status {result}"
            ));
        }
        Ok(())
    }
}

impl Drop for ServiceRegistration {
    fn drop(&mut self) {
        self.deregister(None).ok();
    }
}

fn register(
    instance: ServiceInstance,
    interface_index: Option<NonZeroU32>,
    tx: Option<oneshot::Sender<(u32, ServiceInstance)>>,
) -> Result<(), ServiceRegistrationError> {
    // Hand ownership of the instance to the in-flight registration; it is
    // returned to us through `rx` once the completion callback fires.
    let request = build_request(instance, interface_index, tx);

    // SAFETY: `request` references the instance and context, both owned by the
    // `CallbackContext` behind `context_ptr`, which stays alive until the
    // completion callback reclaims it.
    let result = unsafe { Dns::DnsServiceRegister(&request, None) };
    trace!("DnsServiceRegister result: {result}");

    if result != DNS_REQUEST_PENDING {
        // The callback will not fire; reclaim the context (which frees the
        // instance on drop).
        // SAFETY: `context_ptr` came from `Box::into_raw` in `build_request`.
        let context_ptr = request.pQueryContext as *mut CallbackContext;
        unsafe {
            drop(Box::from_raw(context_ptr));
        }
        return Err(ServiceRegistrationError::RegistrationError(format!(
            "DnsServiceRegister failed with status: {result}"
        )));
    }
    Ok(())
}

/// Builds a `DNS_SERVICE_REGISTER_REQUEST` that transfers ownership of `instance`
/// into a freshly heap-allocated `CallbackContext`.
///
/// Returns the request together with the raw context pointer. The caller is
/// responsible for reclaiming the context: either the completion callback does so
/// once the request goes async, or the caller must `Box::from_raw` it if the
/// underlying DNS-SD call fails to start.
fn build_request(
    instance: ServiceInstance,
    interface_index: Option<NonZeroU32>,
    status_tx: Option<oneshot::Sender<(u32, ServiceInstance)>>,
) -> Dns::DNS_SERVICE_REGISTER_REQUEST {
    let service_instance = instance.as_ptr();
    let context = Box::new(CallbackContext {
        status_tx,
        instance,
    });
    let context_ptr = Box::into_raw(context) as *mut c_void;

    Dns::DNS_SERVICE_REGISTER_REQUEST {
        Version: DNS_QUERY_REQUEST_VERSION1,
        InterfaceIndex: interface_index.map_or(0, |idx| idx.get()),
        pServiceInstance: service_instance,
        pRegisterCompletionCallback: Some(completion_callback),
        pQueryContext: context_ptr,
        ..Default::default()
    }
}

/// Completion callback invoked by the DNS-SD API once a register or deregister
/// request finishes.
extern "system" fn completion_callback(
    status: u32,
    context: *const c_void,
    _service_instance: *const Dns::DNS_SERVICE_INSTANCE,
) {
    // SAFETY: `context` was produced by `Box::into_raw` in `register` /
    // `deregister` and is reclaimed exactly once, here, when the OS invokes the
    // callback for this in-flight operation.
    let ctx = unsafe { Box::from_raw(context as *mut CallbackContext) };
    let CallbackContext {
        status_tx,
        instance,
    } = *ctx;

    if status == ERROR_SUCCESS.0 {
        trace!("DNS-SD operation completed successfully");
    } else {
        error!("DNS-SD operation failed with status: {status}");
    }

    match status_tx {
        // Hand the instance back to the waiting caller, which keeps it (after a
        // successful registration) or drops it to free it (deregistration or
        // failure). If the receiver is gone, the instance is dropped here.
        Some(status_tx) => {
            let _ = status_tx.send((status, instance));
        }
        // Fire-and-forget (drop on deregister): dropping the instance frees it.
        None => drop(instance),
    }
}

fn get_hostname() -> Result<String, String> {
    let mut size: u32 = 0;
    unsafe {
        let _ = GetComputerNameExW(ComputerNameDnsHostname, None, &mut size);
    }

    if size == 0 {
        return Err("hostname empty".to_string());
    }

    let mut buffer = vec![0u16; size as usize];
    unsafe {
        if let Err(err) = GetComputerNameExW(
            ComputerNameDnsHostname,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut size,
        ) {
            Err(format!("GetComputerNameExW failed: {err}"))
        } else {
            Ok(String::from_utf16_lossy(&buffer[..size as usize]))
        }
    }
}
