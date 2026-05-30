// This file contains the raw FFI bindings to the macOS DNS-SD API, as defined in
// `/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include/dns_sd.h`, as
// of version MacOSX26.2.sdk.

pub type DNSServiceFlags = u32;
pub type DNSServiceErrorType = i32;

pub mod error {
    pub type ServiceError = i32;

    pub const NO_ERROR: ServiceError = 0;
    pub const NAME_CONFLICT: ServiceError = -65548;
}

#[repr(C)]
pub struct _DNSServiceRef_t {
    _unused: (),
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

#[repr(transparent)]
#[derive(Debug, Default)]
pub struct DNSServiceRef(pub(crate) *mut _DNSServiceRef_t);

unsafe impl Send for DNSServiceRef {}

pub type DNSServiceRegisterReply = Option<
    unsafe extern "C" fn(
        service_ref: DNSServiceRef,
        flags: DNSServiceFlags,
        error_code: DNSServiceErrorType,
        name: *const ::std::os::raw::c_char,
        regtype: *const ::std::os::raw::c_char,
        domain: *const ::std::os::raw::c_char,
        context: *mut ::std::os::raw::c_void,
    ),
>;

unsafe extern "C" {
    pub unsafe fn DNSServiceRegister(
        sdRef: *mut DNSServiceRef,
        flags: DNSServiceFlags,
        interfaceIndex: u32,
        name: *const ::std::os::raw::c_char,
        regtype: *const ::std::os::raw::c_char,
        domain: *const ::std::os::raw::c_char,
        host: *const ::std::os::raw::c_char,
        port: u16,
        txtLen: u16,
        txtRecord: *const ::std::os::raw::c_void,
        callBack: DNSServiceRegisterReply,
        context: *mut ::std::os::raw::c_void,
    ) -> error::ServiceError;

    pub unsafe fn DNSServiceRefDeallocate(sdRef: DNSServiceRef);

    pub unsafe fn DNSServiceProcessResult(sdRef: *mut _DNSServiceRef_t) -> DNSServiceErrorType;
}
