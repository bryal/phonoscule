//! The single JNI entry point.
//!
//! This is the only `extern` boundary in the project. It does three things and nothing else: read a
//! byte array, hand it to [`crate::proto`], write a byte array back. All logic lives behind it in
//! safe Rust, which is why the whole protocol is testable with `cargo test` on the host.
//!
//! Two hard rules apply here because this is an FFI boundary:
//! - Never unwind into the JVM. Unwinding across FFI is undefined behaviour, so everything runs
//!   inside `catch_unwind`.
//! - Always return a well-formed response. A panic or a failed array conversion still comes back as
//!   an encoded `Response::Error`, never as an exception or a half-built array, so Kotlin always has
//!   something it can parse.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass};

use crate::proto;

/// Set up logging on first call, and send panics to it: Android drops whatever goes to stderr.
fn init_once() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Debug).with_tag("phonoscule"),
        );
        std::panic::set_hook(Box::new(|info| {
            let backtrace = std::backtrace::Backtrace::force_capture();
            log::error!("{info}\n{backtrace}");
        }));
        log::info!("phonoscule core initialised (v{})", env!("CARGO_PKG_VERSION"));
    });
}

/// `zone.jo.phonoscule.Core.call(ByteArray): ByteArray`
///
/// The symbol name encodes the Kotlin package and class, so if you rename either of them on the
/// Kotlin side, you must mirror that here or the library will fail to link at runtime with an
/// `UnsatisfiedLinkError`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_zone_jo_phonoscule_Core_call<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    request: JByteArray<'local>,
) -> JByteArray<'local> {
    init_once();

    let handle_request = AssertUnwindSafe(|| {
        let request = env.convert_byte_array(&request).map_err(|e| format!("failed to read request array: {e}"))?;
        Ok::<Vec<u8>, String>(proto::handle(&request))
    });
    let response: Vec<u8> = match catch_unwind(handle_request) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(message)) => {
            log::error!("jni: {message}");
            proto::encode_error(message)
        }
        Err(payload) => {
            // The panic hook has already logged the payload and backtrace.
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            proto::encode_error(format!("panic in phonoscule core: {message}"))
        }
    };

    env.byte_array_from_slice(&response).unwrap_or_else(|e| {
        // Nothing left to report through: returning null is the only option, and the Kotlin side
        // treats a null return as a hard failure.
        log::error!("jni: failed to allocate response array: {e}");
        JByteArray::default()
    })
}
