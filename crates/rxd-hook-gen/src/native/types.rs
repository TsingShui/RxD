//! ABI projections use structured type kind, width AND actual storage, never C spelling.
use super::{engine::Signature, image::Image};
use crate::{Limits, Result};

pub(super) struct Bindings {
    pub metadata: String,
    pub enter: String,
    pub leave: String,
}
#[cfg(not(feature = "kuna"))]
pub(super) fn bindings(
    _signature: &Signature,
    _image: &Image,
    _limits: Limits,
) -> Result<Bindings> {
    unreachable!("a signature requires the linked engine")
}

#[cfg(feature = "kuna")]
pub(super) fn bindings(signature: &Signature, image: &Image, limits: Limits) -> Result<Bindings> {
    use rxd_kuna_cli::hook::{Scalar, Value};
    let supported_cc = matches!(
        signature.convention.as_str(),
        "__cdecl" | "__stdcall" | "__fastcall" | "default"
    );
    let slot = |value: &Value| -> Option<usize> {
        if !supported_cc || signature.variadic || value.indirect {
            return None;
        }
        let reg = value.register.to_ascii_lowercase();
        match (image.arch, image.platform) {
            ("x64", "linux" | "darwin") => [
                "rdi/edi/di/dil",
                "rsi/esi/si/sil",
                "rdx/edx/dx/dl",
                "rcx/ecx/cx/cl",
                "r8/r8d/r8w/r8b",
                "r9/r9d/r9w/r9b",
            ]
            .iter()
            .position(|aliases| aliases.split('/').any(|alias| alias == reg)),
            ("x64", "windows") => [
                "rcx/ecx/cx/cl",
                "rdx/edx/dx/dl",
                "r8/r8d/r8w/r8b",
                "r9/r9d/r9w/r9b",
            ]
            .iter()
            .position(|aliases| aliases.split('/').any(|alias| alias == reg)),
            ("arm64", _) => (0..8).find(|i| reg == format!("x{i}") || reg == format!("w{i}")),
            ("arm", "linux") => (0..4).find(|i| reg == format!("r{i}")),
            ("ia32", _) if signature.convention == "__cdecl" => value
                .stack_offset
                .filter(|off| *off >= 4 && *off % 4 == 0)
                .map(|off| (off / 4 - 1) as usize),
            _ => None,
        }
    };
    let result_storage = |value: &Value| -> bool {
        if !supported_cc || signature.variadic || value.indirect {
            return false;
        }
        let reg = value.register.to_ascii_lowercase();
        match image.arch {
            "x64" | "ia32" => matches!(reg.as_str(), "rax" | "eax" | "ax" | "al"),
            "arm64" => matches!(reg.as_str(), "x0" | "w0"),
            "arm" => reg == "r0",
            _ => false,
        }
    };
    let convert = |value: &Value, raw: &str| -> Option<(&str, String)> {
        if value.bytes > i32::from(image.pointer_size) || value.bytes <= 0 {
            return None;
        }
        match (&value.scalar, value.bytes) {
            (Scalar::Pointer, width) if width == i32::from(image.pointer_size) => {
                Some(("NativePointer", raw.into()))
            }
            (Scalar::Signed, 1) => Some(("number", format!("({raw}.toInt32() << 24) >> 24"))),
            (Scalar::Signed, 2) => Some(("number", format!("({raw}.toInt32() << 16) >> 16"))),
            (Scalar::Signed, 4) => Some(("number", format!("{raw}.toInt32()"))),
            (Scalar::Unsigned, 1) => Some(("number", format!("{raw}.toUInt32() & 0xff"))),
            (Scalar::Unsigned, 2) => Some(("number", format!("{raw}.toUInt32() & 0xffff"))),
            (Scalar::Unsigned, 4) => Some(("number", format!("{raw}.toUInt32()"))),
            (Scalar::Signed, 8) => Some(("Int64", format!("int64({raw}.toString())"))),
            (Scalar::Unsigned, 8) => Some(("UInt64", format!("uint64({raw}.toString())"))),
            _ => None,
        }
    };
    let fact = |value: &Value| serde_json::json!({"name":value.name,"cType":value.c_type,"bytes":value.bytes,"register":value.register,"stackOffset":value.stack_offset.map(|v|format!("0x{v:x}")),"indirect":value.indirect});
    let metadata = serde_json::json!({"provenance":signature.provenance,"callingConvention":signature.convention,"variadic":signature.variadic,"parameters":signature.parameters.iter().map(fact).collect::<Vec<_>>(),"return":fact(&signature.result)});
    let encoded = crate::source::escape_serialized_json(metadata.to_string());
    limits.check_source(encoded.len())?;
    let mut enter = crate::source::SourceBuilder::new(limits)?;
    for (i, value) in signature.parameters.iter().enumerate() {
        limits.check_target(&value.name)?;
        limits.check_target(&value.c_type)?;
        let safe_name: String = value
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let local = format!("__rxdArg{i}_{safe_name}");
        let raw = slot(value).map(|slot| format!("args[{slot}]"));
        match raw.as_deref().and_then(|raw| convert(value, raw)) {
            Some((ty, expression)) => enter.push(&format!(
                "const {local}: {ty} = {expression};\n__rxdLog(\"argument\", {}, {local});\n",
                crate::source::js_string(&value.name)
            ))?,
            None => {
                // An unsupported storage class (float/vector, hidden/indirect,
                // split-register value, custom CC) is not an args[] value.
                let expression = raw.as_deref().unwrap_or("undefined");
                enter.push(&format!("// decoding unavailable: retain raw general-purpose value only, if its storage is supported.\nconst {local}: NativePointer | undefined = {expression};\n__rxdLog(\"argument decoding unavailable\", {}, {local});\n", crate::source::js_string(&value.name)))?;
            }
        }
    }
    let leave = if matches!(signature.result.scalar, Scalar::Void) {
        "__rxdLog(\"return void\");\n".into()
    } else if let Some((ty, expr)) = result_storage(&signature.result)
        .then(|| convert(&signature.result, "retval"))
        .flatten()
    {
        format!("const __rxdReturn: {ty} = {expr};\n__rxdLog(\"return\", __rxdReturn);\n")
    } else {
        "// Target return decoding unavailable; retval is only Frida's raw general-purpose return view.\nconst __rxdRawReturn: InvocationReturnValue = retval;\n__rxdLog(\"return decoding unavailable\", __rxdRawReturn);\n".into()
    };
    Ok(Bindings {
        metadata: format!(
            "const __rxdSignature = {encoded};\n__rxdLog(\"signature\", __rxdSignature);\n"
        ),
        enter: enter.finish(),
        leave,
    })
}
