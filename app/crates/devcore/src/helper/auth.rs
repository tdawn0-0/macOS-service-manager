//! Peer authentication for the helper server.
//!
//! Two layers, in order: the peer's effective UID must equal the installing
//! UID, then the peer's audit token must satisfy this process's own designated
//! requirement. The first layer is `getpeereid`; the second is
//! `getsockopt(SOL_LOCAL, LOCAL_PEERTOKEN)` feeding
//! `SecCodeCopyGuestWithAttributes` + `SecCodeCheckValidity`.
//!
//! Tests replace [`PeerAuth`]; the production implementation is
//! [`SystemPeerAuth`].

use std::fmt;
use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

/// The peer identity checks the server runs before routing a request.
pub trait PeerAuth: Send + Sync + fmt::Debug {
    /// The effective UID of the peer, or why it could not be read.
    fn peer_uid(&self, stream: &UnixStream) -> io::Result<u32>;
    /// Whether the peer runs code satisfying this helper's designated
    /// requirement; the error text is only logged.
    fn peer_authorized(&self, stream: &UnixStream) -> Result<(), String>;
}

/// The production checks.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemPeerAuth;

impl PeerAuth for SystemPeerAuth {
    fn peer_uid(&self, stream: &UnixStream) -> io::Result<u32> {
        peer_effective_uid(stream)
    }

    fn peer_authorized(&self, stream: &UnixStream) -> Result<(), String> {
        peer_runs_helper_code(stream)
    }
}

/// The connected peer's effective UID via `getpeereid`.
pub fn peer_effective_uid(stream: &UnixStream) -> io::Result<u32> {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn getpeereid(
                fd: libc::c_int,
                euid: *mut libc::uid_t,
                egid: *mut libc::gid_t,
            ) -> libc::c_int;
        }
        let mut euid: libc::uid_t = 0;
        let mut egid: libc::gid_t = 0;
        // SAFETY: the descriptor is open for the stream's lifetime and both
        // out pointers are valid.
        let rc = unsafe { getpeereid(stream.as_raw_fd(), &mut euid, &mut egid) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(euid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = stream;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "getpeereid is only available on macOS",
        ))
    }
}

/// The peer's 32-byte audit token, via `getsockopt(LOCAL_PEERTOKEN)`.
#[cfg(target_os = "macos")]
pub fn peer_audit_token(stream: &UnixStream) -> io::Result<[u8; 32]> {
    let mut token = [0u8; 32];
    let mut length = token.len() as libc::socklen_t;
    // SAFETY: the token buffer is 32 bytes and `length` describes it.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != token.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("peer token length {length}, want {}", token.len()),
        ));
    }
    Ok(token)
}

/// Reports nil only when the peer runs validly signed code that satisfies this
/// process's own designated requirement.
pub fn peer_runs_helper_code(stream: &UnixStream) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let token = peer_audit_token(stream).map_err(|error| error.to_string())?;
        let requirement = self_requirement()?;
        let api = security_api()?;
        let data = api.data_create(&token)?;
        let result = api.code_satisfies(api.audit_key, data, requirement);
        api.release(data);
        result
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = stream;
        Err("peer code checks are only available on macOS".to_string())
    }
}

// ---------------------------------------------------------------------------
// Security.framework / CoreFoundation, loaded lazily like the Go port.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::c_void;
    use std::sync::LazyLock;

    pub type OsStatus = i32;
    type CopySelf = unsafe extern "C" fn(u32, *mut *mut c_void) -> OsStatus;
    type CopyDesignatedRequirement =
        unsafe extern "C" fn(*mut c_void, u32, *mut *mut c_void) -> OsStatus;
    type CopyGuestWithAttributes =
        unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut *mut c_void) -> OsStatus;
    type CheckValidity = unsafe extern "C" fn(*mut c_void, u32, *mut c_void) -> OsStatus;
    type CfRelease = unsafe extern "C" fn(*mut c_void);
    type CfDataCreate = unsafe extern "C" fn(*const c_void, *const u8, isize) -> *mut c_void;
    type CfDictionaryCreate = unsafe extern "C" fn(
        *const c_void,
        *const *const c_void,
        *const *const c_void,
        isize,
        *const c_void,
        *const c_void,
    ) -> *mut c_void;

    pub struct SecurityApi {
        pub copy_self: unsafe extern "C" fn(u32, *mut *mut c_void) -> OsStatus,
        pub copy_designated_requirement:
            unsafe extern "C" fn(*mut c_void, u32, *mut *mut c_void) -> OsStatus,
        pub copy_guest_with_attributes:
            unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut *mut c_void) -> OsStatus,
        pub check_validity: unsafe extern "C" fn(*mut c_void, u32, *mut c_void) -> OsStatus,
        pub release: unsafe extern "C" fn(*mut c_void),
        pub data_create: unsafe extern "C" fn(*const c_void, *const u8, isize) -> *mut c_void,
        pub dictionary_create: unsafe extern "C" fn(
            *const c_void,
            *const *const c_void,
            *const *const c_void,
            isize,
            *const c_void,
            *const c_void,
        ) -> *mut c_void,
        pub key_callbacks: *const c_void,
        pub value_callbacks: *const c_void,
        pub audit_key: *const c_void,
    }

    // The function pointers and CF constants are immutable after loading.
    unsafe impl Send for SecurityApi {}
    unsafe impl Sync for SecurityApi {}

    impl SecurityApi {
        pub fn data_create(&self, bytes: &[u8]) -> Result<*mut c_void, String> {
            // SAFETY: the byte buffer is valid for its length.
            let data = unsafe {
                (self.data_create)(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize)
            };
            if data.is_null() {
                return Err("CFDataCreate failed".to_string());
            }
            Ok(data)
        }

        pub fn release(&self, object: *mut c_void) {
            if !object.is_null() {
                // SAFETY: the object was created by this API.
                unsafe { (self.release)(object) };
            }
        }

        pub fn code_satisfies(
            &self,
            key: *const c_void,
            value: *mut c_void,
            requirement: *mut c_void,
        ) -> Result<(), String> {
            if key.is_null() || value.is_null() || requirement.is_null() {
                return Err("missing code identity inputs".to_string());
            }
            let keys = [key];
            let values = [value];
            // SAFETY: both arrays hold one CF object and the callback pointers
            // came from CoreFoundation.
            let attributes = unsafe {
                (self.dictionary_create)(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr() as *const *const c_void,
                    1,
                    self.key_callbacks,
                    self.value_callbacks,
                )
            };
            if attributes.is_null() {
                return Err("CFDictionaryCreate failed".to_string());
            }
            let mut guest: *mut c_void = std::ptr::null_mut();
            // SAFETY: the attributes dictionary and out pointer are valid.
            let status = unsafe {
                (self.copy_guest_with_attributes)(std::ptr::null_mut(), attributes, 0, &mut guest)
            };
            self.release(attributes);
            if status != 0 || guest.is_null() {
                return Err(format!(
                    "SecCodeCopyGuestWithAttributes failed: OSStatus {status}"
                ));
            }
            // SAFETY: guest is a valid SecCode object and requirement is valid.
            let status = unsafe { (self.check_validity)(guest, 0, requirement) };
            self.release(guest);
            if status != 0 {
                return Err(format!(
                    "peer code does not satisfy requirement: OSStatus {status}"
                ));
            }
            Ok(())
        }

        pub fn copy_self(&self) -> Result<*mut c_void, String> {
            let mut code: *mut c_void = std::ptr::null_mut();
            // SAFETY: the out pointer is valid.
            let status = unsafe { (self.copy_self)(0, &mut code) };
            if status != 0 || code.is_null() {
                return Err(format!("SecCodeCopySelf failed: OSStatus {status}"));
            }
            Ok(code)
        }

        pub fn copy_designated_requirement(
            &self,
            code: *mut c_void,
        ) -> Result<*mut c_void, String> {
            let mut requirement: *mut c_void = std::ptr::null_mut();
            // SAFETY: code is a valid SecCode object.
            let status = unsafe { (self.copy_designated_requirement)(code, 0, &mut requirement) };
            if status != 0 || requirement.is_null() {
                return Err(format!(
                    "SecCodeCopyDesignatedRequirement failed: OSStatus {status}"
                ));
            }
            Ok(requirement)
        }
    }

    static API: LazyLock<Result<SecurityApi, String>> = LazyLock::new(load);

    pub fn security_api() -> Result<&'static SecurityApi, String> {
        API.as_ref().map_err(Clone::clone)
    }

    fn load() -> Result<SecurityApi, String> {
        use std::ffi::{CStr, c_char, c_int};

        const RTLD_LAZY: c_int = 1;
        const RTLD_GLOBAL: c_int = 0x100;

        unsafe extern "C" {
            fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
            fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        }

        unsafe fn symbol(handle: *mut c_void, name: &CStr) -> Result<*mut c_void, String> {
            // SAFETY: the handle is open and name is NUL-terminated.
            let symbol = unsafe { dlsym(handle, name.as_ptr()) };
            if symbol.is_null() {
                return Err(format!("dlsym {} failed", name.to_string_lossy()));
            }
            Ok(symbol)
        }

        // SAFETY: paths are valid C strings; failure is reported below.
        let security = unsafe {
            dlopen(
                c"/System/Library/Frameworks/Security.framework/Security".as_ptr(),
                RTLD_LAZY | RTLD_GLOBAL,
            )
        };
        if security.is_null() {
            return Err("could not load Security.framework".to_string());
        }
        // SAFETY: same as above.
        let core_foundation = unsafe {
            dlopen(
                c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation".as_ptr(),
                RTLD_LAZY | RTLD_GLOBAL,
            )
        };
        if core_foundation.is_null() {
            return Err("could not load CoreFoundation.framework".to_string());
        }

        // SAFETY: each symbol name matches the documented signature.
        unsafe {
            let copy_self = symbol(security, c"SecCodeCopySelf")?;
            let copy_designated_requirement =
                symbol(security, c"SecCodeCopyDesignatedRequirement")?;
            let copy_guest_with_attributes = symbol(security, c"SecCodeCopyGuestWithAttributes")?;
            let check_validity = symbol(security, c"SecCodeCheckValidity")?;
            let release = symbol(core_foundation, c"CFRelease")?;
            let data_create = symbol(core_foundation, c"CFDataCreate")?;
            let dictionary_create = symbol(core_foundation, c"CFDictionaryCreate")?;
            let key_callbacks = symbol(core_foundation, c"kCFTypeDictionaryKeyCallBacks")?;
            let value_callbacks = symbol(core_foundation, c"kCFTypeDictionaryValueCallBacks")?;
            let audit_key_symbol = symbol(security, c"kSecGuestAttributeAudit")?;
            let audit_key = *(audit_key_symbol as *const *const c_void);
            if audit_key.is_null() {
                return Err("kSecGuestAttributeAudit is nil".to_string());
            }
            Ok(SecurityApi {
                copy_self: std::mem::transmute::<*mut c_void, CopySelf>(copy_self),
                copy_designated_requirement: std::mem::transmute::<
                    *mut c_void,
                    CopyDesignatedRequirement,
                >(copy_designated_requirement),
                copy_guest_with_attributes: std::mem::transmute::<
                    *mut c_void,
                    CopyGuestWithAttributes,
                >(copy_guest_with_attributes),
                check_validity: std::mem::transmute::<*mut c_void, CheckValidity>(check_validity),
                release: std::mem::transmute::<*mut c_void, CfRelease>(release),
                data_create: std::mem::transmute::<*mut c_void, CfDataCreate>(data_create),
                dictionary_create: std::mem::transmute::<*mut c_void, CfDictionaryCreate>(
                    dictionary_create,
                ),
                key_callbacks: key_callbacks as *const c_void,
                value_callbacks: value_callbacks as *const c_void,
                audit_key,
            })
        }
    }
}

#[cfg(target_os = "macos")]
pub use sys::security_api;

/// This process's designated requirement, computed once and intentionally
/// never released (matching the Go helper).
#[cfg(target_os = "macos")]
pub fn self_requirement() -> Result<*mut std::ffi::c_void, String> {
    use std::sync::LazyLock;
    static REQUIREMENT: LazyLock<Result<usize, String>> = LazyLock::new(|| {
        let api = security_api()?;
        let code = api.copy_self()?;
        let requirement = api.copy_designated_requirement(code);
        api.release(code);
        requirement.map(|requirement| requirement as usize)
    });
    REQUIREMENT
        .clone()
        .map(|requirement| requirement as *mut std::ffi::c_void)
}

// ---------------------------------------------------------------------------
// Test-only checks: the same APIs the Go tests used to prove self acceptance.
// ---------------------------------------------------------------------------

/// Whether the process with `pid` runs code satisfying this process's
/// designated requirement. Used by tests to prove accept/reject behavior.
#[cfg(all(test, target_os = "macos"))]
pub fn pid_satisfies_self(pid: i32) -> Result<(), String> {
    use std::ffi::c_void;
    use std::sync::LazyLock;

    type CfNumberCreate = unsafe extern "C" fn(*const c_void, i32, *const i32) -> *mut c_void;
    unsafe extern "C" {
        fn dlopen(path: *const std::ffi::c_char, mode: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
    }

    static CREATE_NUMBER: LazyLock<CfNumberCreate> = LazyLock::new(|| {
        // SAFETY: the framework path and symbol are valid.
        unsafe {
            let handle = dlopen(
                c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation".as_ptr(),
                1,
            );
            let symbol = dlsym(handle, c"CFNumberCreate".as_ptr());
            std::mem::transmute::<*mut c_void, CfNumberCreate>(symbol)
        }
    });

    static PID_KEY: LazyLock<usize> = LazyLock::new(|| {
        // SAFETY: the framework path and symbol are valid.
        unsafe {
            let handle = dlopen(
                c"/System/Library/Frameworks/Security.framework/Security".as_ptr(),
                1,
            );
            let symbol = dlsym(handle, c"kSecGuestAttributePid".as_ptr());
            *(symbol as *const *const c_void) as usize
        }
    });
    let key = *PID_KEY;
    let api = security_api()?;
    let requirement = self_requirement()?;
    // SAFETY: CFNumberCreate returns a +1 CFNumber for a valid int32.
    let number = unsafe { CREATE_NUMBER(std::ptr::null(), 3, &pid) };
    if number.is_null() {
        return Err("CFNumberCreate failed".to_string());
    }
    let result = api.code_satisfies(key as *const c_void, number, requirement);
    api.release(number);
    result
}

// Keep the unused import warning away on non-macOS targets.
#[cfg(not(target_os = "macos"))]
fn _unused(_: &UnixStream) {}
