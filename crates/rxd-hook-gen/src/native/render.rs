use super::{Request, image::Image};
use crate::source::{
    SourceBuilder, StageEvent, js_string, setup_catch, setup_prelude, setup_stage,
};
use crate::{Artifact, Language, Limits, Result, SourceArtifact, Stage};

pub(super) fn render(request: &Request, image: &Image, limits: Limits) -> Result<Artifact> {
    let ts = request.options.language == Language::TypeScript;
    let mut out = SourceBuilder::new(limits)?;
    out.push("(() => {\n")?;
    out.push(&setup_prelude(&image.label, request.options.language))?;
    out.push(&format!(
        "const __rxdSourceId = {};\n",
        js_string(request.input.source_id())
    ))?;
    out.push("try {\n")?;
    out.push(&setup_stage(Stage::NativeModule, StageEvent::Begin))?;
    // Snapshot runtime facts locally: direct Process property guards narrow the
    // ambient global across IIFEs in TS and break cross-target composition.
    out.push("const __rxdArch = Process.arch;\nconst __rxdPointerSize = Process.pointerSize;\nconst __rxdPlatform = Process.platform;\n")?;
    out.push(&format!("if (__rxdArch !== {} || __rxdPointerSize !== {} || __rxdPlatform !== {}) throw new Error(\"runtime architecture/platform does not match analyzed image\");\n", js_string(image.arch), image.pointer_size, js_string(image.platform)))?;
    let module = match &image.module {
        None => "Process.mainModule".to_owned(),
        Some(name) => format!("Process.getModuleByName({})", js_string(name)),
    };
    out.push(&format!("const __rxdModule = {module};\n"))?;
    if let Some(name) = &image.main_name {
        out.push(&format!("if (__rxdModule.name !== {}) throw new Error(\"main module name does not match captured input; use explicit module selection for a renamed executable\");\n", js_string(name)))?;
    }
    if !image.page_mappings.is_empty() {
        out.push(&format!("if (!ptr(\"0x{:x}\").and(Process.pageSize - 1).isNull()) throw new Error(\"ELF image base is not aligned to runtime page size\");\n", image.base))?;
        for (vma, offset) in &image.page_mappings {
            out.push(&format!("if (!ptr(\"0x{vma:x}\").and(Process.pageSize - 1).equals(ptr(\"0x{offset:x}\").and(Process.pageSize - 1))) throw new Error(\"ELF mapping is incompatible with runtime page size\");\n"))?;
        }
    }
    out.push(&setup_stage(Stage::NativeModule, StageEvent::Success))?;
    out.push(&setup_stage(Stage::NativeTarget, StageEvent::Begin))?;
    let annotation = if ts { ": NativePointer" } else { "" };
    out.push(&format!(
        "const __rxdAddress{annotation} = __rxdModule.base.add(\"0x{:x}\");\n",
        image.address - image.base
    ))?;
    out.push("if (__rxdAddress.compare(__rxdModule.base) < 0 || __rxdAddress.compare(__rxdModule.base.add(__rxdModule.size)) >= 0) throw new Error(\"target is outside runtime module\");\nconst __rxdRange = Process.findRangeByAddress(__rxdAddress);\nif (__rxdRange === null || !__rxdRange.protection.includes(\"x\")) throw new Error(\"target runtime mapping is not executable\");\n")?;
    out.push(&format!(
        "const __rxdAttachAddress{annotation} = __rxdAddress{};\n",
        if image.thumb { ".or(1)" } else { "" }
    ))?;
    out.push(&setup_stage(Stage::NativeTarget, StageEvent::Success))?;
    let bindings = if ts {
        image
            .signature
            .as_ref()
            .map(|signature| super::types::bindings(signature, image, limits))
            .transpose()?
    } else {
        None
    };
    if let Some(bindings) = &bindings {
        out.push(&bindings.metadata)?;
    } else {
        out.push(
            "// signature: unknown; argument count and ABI are not invented. Values remain raw.\n",
        )?;
    }
    out.push(&setup_stage(Stage::NativeAttach, StageEvent::Begin))?;
    if ts {
        out.push("Interceptor.attach(__rxdAttachAddress, {\n  onEnter(this: InvocationContext, args: InvocationArguments): void {\n    __rxdLog(\"call\", args);\n")?;
        if let Some(bindings) = &bindings {
            out.push(&bindings.enter)?;
        }
        out.push(
            "  },\n  onLeave(this: InvocationContext, retval: InvocationReturnValue): void {\n",
        )?;
        if let Some(bindings) = &bindings {
            out.push(&bindings.leave)?;
        } else {
            out.push("__rxdLog(\"return\", retval);\n")?;
        }
        out.push("  }\n});\n")?;
    } else {
        out.push(
        "Interceptor.attach(__rxdAttachAddress, {\n  onEnter(args) {\n    __rxdLog(\"call\", args);\n  },\n  onLeave(retval) {\n    __rxdLog(\"return\", retval);\n  }\n});\n"
    )?;
    }
    out.push(&setup_stage(Stage::NativeAttach, StageEvent::Success))?;
    out.push("}\n")?;
    out.push(setup_catch())?;
    out.push("})();\n")?;
    Ok(Artifact::Source(SourceArtifact::new(
        request.options.language,
        out.finish(),
        limits,
    )?))
}
