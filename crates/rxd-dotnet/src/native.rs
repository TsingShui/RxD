//! Load the packaged engine on demand without a mandatory startup dependency.
use std::{path::PathBuf, sync::OnceLock};

use anyhow::{Result, ensure};
use serde::Deserialize;

use crate::{
    ClassBody, ClassDecompilation, ClassSourceMap, Decompilation, Input, Method, MethodBody,
    SourceMap,
    decompile::{class_fallback, fallback},
};

type DecompileMethod = unsafe extern "C" fn(*const u8, i32, i32, *mut i32) -> *mut u8;
type Free = unsafe extern "C" fn(*mut u8);

struct Engine {
    _library: libloading::Library,
    _private_openssl: Option<libloading::Library>,
    method: DecompileMethod,
    class: Option<DecompileMethod>,
    free: Free,
}

#[cfg(feature = "bundled-ilspy")]
mod embed {
    include!(env!("RXD_ILSPY_EMBED_RS"));
}

fn library_path() -> Result<Option<PathBuf>> {
    if let Some(path) = std::env::var_os("RXD_DOTNET_NATIVE_LIBRARY") {
        return Ok(Some(path.into()));
    }
    #[cfg(feature = "bundled-ilspy")]
    {
        let manifest: rxd_core::bundle::Manifest = serde_json::from_str(embed::MANIFEST)?;
        let cache =
            rxd_core::bundle::materialize("rxd-ilspy", embed::DATA, embed::TARGET, &manifest)
                .map_err(|error| anyhow::anyhow!("ILSPY_BUNDLE_ERROR: {error}"))?;
        Ok(Some(cache.join(manifest.executable.ok_or_else(|| {
            anyhow::anyhow!("ILSpy bundle has no library")
        })?)))
    }
    #[cfg(not(feature = "bundled-ilspy"))]
    Ok(external_library_path())
}

#[cfg(not(feature = "bundled-ilspy"))]
fn external_library_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let directory = executable.parent()?;
    let adjacent = directory.join(env!("RXD_DOTNET_LIBRARY_NAME"));
    if adjacent.exists()
        || directory.canonicalize().ok()?
            != std::path::Path::new(env!("RXD_DOTNET_BUILD_DIR"))
                .canonicalize()
                .ok()?
    {
        return Some(adjacent);
    }
    // Local Cargo builds can use the prepared library without copying it.
    // Relocated distributions must never fall back to a build-machine path.
    Some(env!("RXD_DOTNET_NATIVE_LIBRARY").into())
}

/// Maximum response size of the pinned native ABI.
const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

impl Engine {
    fn load() -> std::result::Result<Option<Self>, String> {
        library_path()
            .map(|path| path.and_then(Self::open))
            .map_err(|error| error.to_string())
    }

    fn open(path: PathBuf) -> Option<Self> {
        let path = path.canonicalize().ok()?;
        // The Android bundle links its own OpenSSL. Android's linker does not
        // search the extraction directory, so load the private library first.
        let private_openssl = path.parent()?.join("librxdopenssl.so");
        let private_openssl = if private_openssl.is_file() {
            // SAFETY: the extracted library is part of the verified bundle.
            Some(unsafe { libloading::Library::new(&private_openssl).ok()? })
        } else {
            None
        };
        // SAFETY: the explicitly selected or installed library is trusted and
        // implements this C ABI. Retain its handle with the function pointers.
        unsafe {
            let library = libloading::Library::new(&path).ok()?;
            let method = *library
                .get::<DecompileMethod>(b"rxd_decompile_method\0")
                .ok()?;
            let free = *library.get::<Free>(b"rxd_free\0").ok()?;
            let class = library
                .get::<DecompileMethod>(b"rxd_decompile_class\0")
                .ok()
                .map(|symbol| *symbol);
            // Optional initialization keeps older explicitly supplied engines compatible.
            type SetReferences = unsafe extern "C" fn(*const u8, i32) -> i32;
            if let Ok(configure) = library.get::<SetReferences>(b"rxd_set_reference_directory\0") {
                let references = path.parent()?.join("dotnet-refs");
                let encoded = references.to_str()?.as_bytes();
                if configure(encoded.as_ptr(), i32::try_from(encoded.len()).ok()?) != 1 {
                    return None;
                }
            }
            Some(Self {
                method,
                class,
                free,
                _library: library,
                _private_openssl: private_openssl,
            })
        }
    }
}

#[derive(Deserialize)]
struct Payload {
    ok: bool,
    reason: Option<String>,
    code: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    mapping: Vec<NativeSourceMap>,
}

/// Keep proven mapping entries and count rejected ones without discarding source.
fn retain_mapping<T>(mapping: Vec<T>, keep: impl Fn(&T) -> bool) -> (Vec<T>, usize) {
    let before = mapping.len();
    let kept: Vec<T> = mapping.into_iter().filter(|site| keep(site)).collect();
    let dropped = before - kept.len();
    (kept, dropped)
}

#[derive(Deserialize)]
struct NativeSourceMap {
    method_token: Option<u32>,
    il_offset: u32,
    line: usize,
}

enum DecodedPayload {
    Code {
        code: String,
        warnings: Vec<String>,
        mapping: Vec<NativeSourceMap>,
    },
    Fallback(String),
}

impl Engine {
    fn decode_payload(&self, ptr: *mut u8, out_len: i32) -> Result<DecodedPayload> {
        if ptr.is_null() {
            return Ok(DecodedPayload::Fallback("engine_failure".into()));
        }
        // The matching ABI allocator owns this payload. Free it exactly once,
        // including malformed JSON, invalid lengths and adjustable cap failures.
        let payload = if out_len > 0 && out_len as usize <= MAX_PAYLOAD_BYTES {
            // SAFETY: the trusted ABI allocation contains out_len readable bytes.
            serde_json::from_slice::<Payload>(unsafe {
                std::slice::from_raw_parts(ptr, out_len as usize)
            })
            .ok()
        } else {
            None
        };
        // SAFETY: ptr came from this engine's matching allocator.
        unsafe {
            (self.free)(ptr);
        }
        ensure!(
            out_len <= 0 || out_len as usize <= MAX_PAYLOAD_BYTES,
            "UNSUPPORTED_INPUT: the native engine returned more than one ABI payload can carry"
        );
        let Some(payload) = payload else {
            return Ok(DecodedPayload::Fallback("invalid_engine_output".into()));
        };
        if !payload.ok {
            ensure!(
                !matches!(
                    payload.reason.as_deref(),
                    Some("input_limit" | "output_limit")
                ),
                "RESOURCE_LIMIT: native decompilation {}",
                payload.reason.as_deref().unwrap_or("limit")
            );
            return Ok(DecodedPayload::Fallback(
                payload
                    .reason
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or_else(|| "engine_failure".into()),
            ));
        }
        let Some(code) = payload.code.filter(|code| !code.trim().is_empty()) else {
            return Ok(DecodedPayload::Fallback("invalid_engine_output".into()));
        };
        if payload.reason.is_some() {
            return Ok(DecodedPayload::Fallback("invalid_engine_output".into()));
        }
        Ok(DecodedPayload::Code {
            code,
            warnings: payload.warnings,
            mapping: payload.mapping,
        })
    }
}

pub(super) fn render(
    input: &Input<'_>,
    method: &Method,
    body: &MethodBody,
) -> Result<Decompilation> {
    static ENGINE: OnceLock<std::result::Result<Option<Engine>, String>> = OnceLock::new();
    let Some(engine) = ENGINE
        .get_or_init(Engine::load)
        .as_ref()
        .map_err(anyhow::Error::msg)?
    else {
        return Ok(fallback("engine_unavailable"));
    };
    let bytes = input.bytes;
    ensure!(
        bytes.len() <= 64 * 1024 * 1024,
        "RESOURCE_LIMIT: native input byte cap"
    );
    let Ok(length) = i32::try_from(bytes.len()) else {
        return Ok(fallback("input_limit"));
    };
    let mut out_len = 0;
    // SAFETY: input.view owns the immutable, validated module buffer throughout
    // the call; out_len is writable; method is a validated MethodDef token.
    let ptr = unsafe { (engine.method)(bytes.as_ptr(), length, method.token as i32, &mut out_len) };
    let (code, warnings, mapping) = match engine.decode_payload(ptr, out_len)? {
        DecodedPayload::Code {
            code,
            warnings,
            mapping,
        } => (code, warnings, mapping),
        DecodedPayload::Fallback(reason) => return Ok(fallback(reason)),
    };
    // Navigation entries that cannot be proven are dropped and reported; the engine's
    // reconstruction itself is always returned as produced.
    let lines = code.lines().count();
    let mut warnings = warnings;
    let (mapping, dropped) = retain_mapping(mapping, |site| {
        !site.method_token.is_some_and(|token| token != method.token)
            && site.line != 0
            && site.line <= lines
            && body
                .instructions
                .binary_search_by_key(&site.il_offset, |instruction| instruction.offset)
                .is_ok()
    });
    if dropped != 0 {
        warnings.push(format!(
            "mapping: dropped {dropped} unprovable source entry/entries"
        ));
    }
    Ok(Decompilation {
        representation: "csharp".into(),
        decompiled: true,
        reason: None,
        warnings,
        code: Some(code),
        mapping: mapping
            .into_iter()
            .map(|site| SourceMap {
                il_offset: site.il_offset,
                line: site.line,
            })
            .collect(),
    })
}

pub(super) fn render_class(input: &Input<'_>, class: &ClassBody<'_>) -> Result<ClassDecompilation> {
    static ENGINE: OnceLock<std::result::Result<Option<Engine>, String>> = OnceLock::new();
    let Some(engine) = ENGINE
        .get_or_init(Engine::load)
        .as_ref()
        .map_err(anyhow::Error::msg)?
    else {
        return Ok(class_fallback("class_engine_unavailable"));
    };
    let Some(endpoint) = engine.class else {
        return Ok(class_fallback("class_abi_unavailable"));
    };
    let bytes = input.bytes;
    ensure!(
        bytes.len() <= 64 * 1024 * 1024,
        "RESOURCE_LIMIT: native input byte cap"
    );
    let length = i32::try_from(bytes.len())?;
    let mut out_len = 0;
    // SAFETY: the fixed module buffer stays alive; class.root is a validated TypeDef.
    let ptr = unsafe {
        endpoint(
            bytes.as_ptr(),
            length,
            class.root.token as i32,
            &mut out_len,
        )
    };
    let (code, warnings, sites) = match engine.decode_payload(ptr, out_len)? {
        DecodedPayload::Code {
            code,
            warnings,
            mapping,
        } => (code, warnings, mapping),
        DecodedPayload::Fallback(reason) => return Ok(class_fallback(reason)),
    };
    // Same rule as a single body: keep every provable entry, report the rest, and return
    // the reconstruction no matter what the mapping looks like.
    let lines = code.lines().count();
    let mut warnings = warnings;
    let mut mapping = Vec::new();
    let mut dropped = 0usize;
    for site in sites {
        let resolved = site.method_token.and_then(|token| {
            let index = class
                .methods
                .binary_search_by_key(&token, |item| item.method.token)
                .ok()?;
            let item = &class.methods[index];
            let body = item.body.as_ref()?;
            (site.line != 0
                && site.line <= lines
                && body
                    .instructions
                    .binary_search_by_key(&site.il_offset, |instruction| instruction.offset)
                    .is_ok())
            .then_some(token)
        });
        match resolved {
            Some(token) => mapping.push(ClassSourceMap {
                method_token: token,
                il_offset: site.il_offset,
                line: site.line,
            }),
            None => dropped += 1,
        }
    }
    if dropped != 0 {
        warnings.push(format!(
            "mapping: dropped {dropped} unprovable source entry/entries"
        ));
    }
    mapping.sort_unstable_by_key(|site| (site.method_token, site.il_offset, site.line));
    mapping.dedup_by_key(|site| (site.method_token, site.il_offset, site.line));
    Ok(ClassDecompilation {
        representation: "csharp".into(),
        decompiled: true,
        reason: None,
        warnings,
        code: Some(code),
        mapping,
    })
}
